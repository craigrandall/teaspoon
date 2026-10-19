//! The staging area of the streamed export (M4h-0, option C: stage, then commit).
//!
//! The export used to render every file of the archive in memory, compare each with the target,
//! hash them all, write them, and read them back. Memory therefore grew with the archive, and
//! the owner's PST files are several gigabytes. Now each file is written to the staging
//! directory (`<out>/.tsp-tmp`, under a short numeric name) the moment it is rendered, and only
//! a manifest is kept in memory: the relative path, the size, and the SHA-256 of each file
//! (about 100 to 150 bytes per file). The manifest answers every question the in-memory list
//! answered: the preflight compares sizes and hashes with the target, the tree hash is computed
//! from the paths and hashes, and the read-back streams each written file through the hasher.
//!
//! Rendered text goes in whole through [`Stager::add`]. Attachment data goes in through
//! [`Stager::add_stream`] (M4f), which copies a source stream in fixed-size chunks and hashes as
//! it writes, so memory is one chunk however large the attachment is.
//!
//! After consent, [`Stager::commit`] renames the staged files into place (same volume, so
//! nothing is copied) and deletes the ones identical to what is already there. A refused or
//! failed run calls [`Stager::discard`] (or leaves numeric files that the next run removes).
//! Nothing here prints; nothing here names a file in an error message.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::archive::FOLDER_JSON_NAME;

/// Staging directory directly under `--out`. Files are written here under short numeric names
/// and then renamed into place, so a half-written file never appears at its final path.
pub(crate) const STAGING_DIR: &str = ".tsp-tmp";
/// `<out>\.tsp-tmp\<up to 10 digits>`: the longest staged path is `out + 1 + 8 + 1 + 10`.
pub(crate) const STAGING_TAIL_UNITS: usize = 1 + 8 + 1 + 10;

/// Chunk size for copying a source stream into staging.
pub(crate) const COPY_CHUNK_BYTES: usize = 256 * 1024;

/// Why a source stream could not be staged. The source is at fault, not the staging area, so the
/// export goes on and records the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceFailure {
    /// A read from the source failed part-way.
    ReadError,
    /// The bytes read were not the length the source declared.
    LengthMismatch,
}

/// What [`Stager::add_stream`] did with a source stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamOutcome {
    /// The whole stream was staged; `size` and `digest` are what was actually written.
    Staged { size: u64, digest: [u8; 32] },
    /// The source failed; nothing was staged.
    SourceFailed(SourceFailure),
}

pub(crate) fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

pub(crate) fn join_path(root: &Path, rel: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in rel {
        path.push(part);
    }
    path
}

/// The SHA-256 and size of a file, read in chunks (a file is never held whole).
pub(crate) fn hash_file(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total += n as u64;
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hasher.finalize());
    Ok((digest, total))
}

/// What the manifest knows about one staged file. The content is not kept.
#[derive(Debug, Clone)]
pub(crate) struct StagedFile {
    /// Relative to the archive root.
    pub(crate) rel: Vec<String>,
    pub(crate) size: u64,
    pub(crate) digest: [u8; 32],
    /// The number the file has under `.tsp-tmp`.
    staged: u64,
}

impl StagedFile {
    fn is_root(&self) -> bool {
        self.rel.len() == 1 && self.rel[0] == FOLDER_JSON_NAME
    }
}

/// How a staged file compares with what the target already holds (decided by the preflight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    Create,
    Replace,
    Identical,
}

pub(crate) struct Stager {
    staging: PathBuf,
    counter: u64,
    files: Vec<StagedFile>,
    /// The copy buffer [`Stager::add_stream`] reuses; empty until the first stream.
    scratch: Vec<u8>,
}

/// Empties a staging directory of the numeric files an interrupted run left behind, creating it
/// if it does not exist. Anything else in it is not ours: stop.
fn prepare_staging(staging: &Path) -> Result<()> {
    match std::fs::read_dir(staging) {
        Ok(entries) => {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
                let leftover =
                    is_file && !name.is_empty() && name.chars().all(|c| c.is_ascii_digit());
                if !leftover {
                    bail!(
                        "the staging directory .tsp-tmp already holds entries this tool did not create; remove it and retry"
                    );
                }
                std::fs::remove_file(entry.path())
                    .context("failed to remove a leftover staged file")?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(staging).context("failed to create the staging directory")
        }
        Err(_) => bail!("the staging path .tsp-tmp exists but is not a readable directory"),
    }
}

fn move_into_place(staged: &Path, final_path: &Path) -> Result<()> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent).context("failed to create an output directory")?;
    }
    // On Windows `rename` replaces an existing file (MoveFileExW with MOVEFILE_REPLACE_EXISTING).
    std::fs::rename(staged, final_path).context("failed to move a staged file into place")
}

impl Stager {
    /// Creates `out` if needed and prepares an empty staging directory under it.
    pub(crate) fn begin(out: &Path) -> Result<Stager> {
        std::fs::create_dir_all(out).context("failed to create the output directory")?;
        let staging = out.join(STAGING_DIR);
        prepare_staging(&staging)?;
        Ok(Stager {
            staging,
            counter: 0,
            files: Vec::new(),
            scratch: Vec::new(),
        })
    }

    /// Writes one file to the staging directory and records it in the manifest. The content is
    /// not kept.
    pub(crate) fn add(&mut self, rel: Vec<String>, content: &[u8]) -> Result<()> {
        self.counter += 1;
        let id = self.counter;
        std::fs::write(self.staging.join(id.to_string()), content)
            .context("failed to write a staged file")?;
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&Sha256::digest(content));
        self.files.push(StagedFile {
            rel,
            size: content.len() as u64,
            digest,
            staged: id,
        });
        Ok(())
    }

    /// Copies `source` to the staging directory in chunks and records it in the manifest,
    /// hashing as it writes: memory is one chunk, whatever the size. `declared_len`, when the
    /// source knows it, is checked against the bytes actually read.
    ///
    /// A failure of the source (a read error, a wrong length) removes the partial staged file,
    /// adds nothing to the manifest, does not use up a staged number, and returns
    /// `Ok(SourceFailed(..))`. A failure to write to staging is an `Err`: the export stops.
    pub(crate) fn add_stream(
        &mut self,
        rel: Vec<String>,
        source: &mut dyn Read,
        declared_len: Option<u64>,
    ) -> Result<StreamOutcome> {
        let id = self.counter + 1;
        let path = self.staging.join(id.to_string());
        let mut file = std::fs::File::create(&path).context("failed to create a staged file")?;
        if self.scratch.len() != COPY_CHUNK_BYTES {
            self.scratch = vec![0u8; COPY_CHUNK_BYTES];
        }
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut failure = None;
        loop {
            let n = match source.read(&mut self.scratch) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    failure = Some(SourceFailure::ReadError);
                    break;
                }
            };
            if let Err(e) = file.write_all(&self.scratch[..n]) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(anyhow::Error::new(e).context("failed to write a staged file"));
            }
            hasher.update(&self.scratch[..n]);
            total += n as u64;
        }
        if failure.is_none() && declared_len.is_some_and(|declared| declared != total) {
            failure = Some(SourceFailure::LengthMismatch);
        }
        drop(file);
        if let Some(failure) = failure {
            let _ = std::fs::remove_file(&path);
            return Ok(StreamOutcome::SourceFailed(failure));
        }
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&hasher.finalize());
        self.counter = id;
        self.files.push(StagedFile {
            rel,
            size: total,
            digest,
            staged: id,
        });
        Ok(StreamOutcome::Staged {
            size: total,
            digest,
        })
    }

    /// Sorts the manifest by relative path. Call once, after the last `add` and before anything
    /// else: the tree hash, the preflight, and the commit all use this order.
    pub(crate) fn finish(&mut self) {
        self.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    }

    pub(crate) fn files(&self) -> &[StagedFile] {
        &self.files
    }

    /// One hash over all relative paths and file hashes, so two exports can be compared by a
    /// single line of output. The same value the in-memory export computed (path, a zero byte,
    /// the lower-case hex SHA-256 of the content, a line feed, in path order).
    pub(crate) fn tree_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for file in &self.files {
            hasher.update(file.rel.join("/").as_bytes());
            hasher.update([0u8]);
            hasher.update(hex(file.digest).as_bytes());
            hasher.update(b"\n");
        }
        hex(hasher.finalize())
    }

    fn clear(&mut self) {
        for file in &self.files {
            let _ = std::fs::remove_file(self.staging.join(file.staged.to_string()));
        }
        // Best effort: an empty directory is all that can be left.
        let _ = std::fs::remove_dir(&self.staging);
    }

    /// Deletes every staged file and the staging directory (a refused or failed run).
    pub(crate) fn discard(mut self) {
        self.clear();
    }

    /// Moves the staged files into place. `dispositions` is aligned with [`Stager::files`];
    /// `changes` is how many files the preflight found to create or replace. With none, nothing
    /// is touched in the target. Otherwise the root `folder.json` is written first as
    /// `incomplete_root`, every file that is not identical is renamed into place, the identical
    /// ones are deleted from staging, and the staged complete root replaces the incomplete one
    /// last, so an interrupted run is recognizable. Returns (files written, files unchanged).
    pub(crate) fn commit(
        &mut self,
        root_dir: &Path,
        incomplete_root: &[u8],
        dispositions: &[Disposition],
        changes: usize,
    ) -> Result<(usize, usize)> {
        if changes == 0 {
            let unchanged = self.files.len();
            self.clear();
            return Ok((0, unchanged));
        }

        let root_path = root_dir.join(FOLDER_JSON_NAME);
        self.counter += 1;
        let incomplete_staged = self.staging.join(self.counter.to_string());
        std::fs::write(&incomplete_staged, incomplete_root)
            .context("failed to write a staged file")?;
        move_into_place(&incomplete_staged, &root_path)?;

        let mut written = 0usize;
        let mut unchanged = 0usize;
        let mut root_staged: Option<PathBuf> = None;
        for (index, file) in self.files.iter().enumerate() {
            let staged = self.staging.join(file.staged.to_string());
            if file.is_root() {
                root_staged = Some(staged);
                continue;
            }
            let disposition = dispositions
                .get(index)
                .copied()
                .unwrap_or(Disposition::Replace);
            if disposition == Disposition::Identical {
                let _ = std::fs::remove_file(&staged);
                unchanged += 1;
            } else {
                move_into_place(&staged, &join_path(root_dir, &file.rel))?;
                written += 1;
            }
        }
        if let Some(staged) = root_staged {
            move_into_place(&staged, &root_path)?;
            written += 1;
        }
        let _ = std::fs::remove_dir(&self.staging);
        Ok((written, unchanged))
    }

    /// Streams every file the manifest lists through the hasher from its final place and counts
    /// those that are missing, of another size, or of another hash.
    pub(crate) fn read_back_mismatches(&self, root_dir: &Path) -> usize {
        self.files
            .iter()
            .filter(|file| match hash_file(&join_path(root_dir, &file.rel)) {
                Ok((digest, size)) => digest != file.digest || size != file.size,
                Err(_) => true,
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-stage-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn rel(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn staging_names(out: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(out.join(STAGING_DIR))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn hex_encodes_lowercase_with_leading_zeros() {
        assert_eq!(hex([0x00u8, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn staged_files_are_written_under_numbers_and_listed_in_path_order() {
        let dir = temp_dir("order");
        let mut stager = Stager::begin(&dir.join("out")).expect("begin");
        stager.add(rel(&["b", "message.md"]), b"two").unwrap();
        stager.add(rel(&["a", "message.md"]), b"one").unwrap();
        stager.finish();
        let listed: Vec<String> = stager.files().iter().map(|f| f.rel.join("/")).collect();
        assert_eq!(listed, vec!["a/message.md", "b/message.md"]);
        assert_eq!(stager.files()[0].size, 3);
        assert_eq!(staging_names(&dir.join("out")), vec!["1", "2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tree_hash_is_the_documented_formula_and_depends_on_paths_and_content() {
        let dir = temp_dir("treehash");
        let build = |tag: &str, path: &[&str], content: &[u8]| {
            let mut stager = Stager::begin(&dir.join(tag)).expect("begin");
            stager.add(rel(path), content).unwrap();
            stager.finish();
            stager.tree_hash()
        };
        let a = build("a", &["x", "message.md"], b"one");
        assert_eq!(a, build("a2", &["x", "message.md"], b"one"));
        assert_ne!(a, build("b", &["x", "message.md"], b"two"));
        assert_ne!(a, build("c", &["y", "message.md"], b"one"));
        assert_eq!(a.len(), 64);

        // The formula, written out independently: path, 0 byte, hex SHA-256 of the content, LF.
        let mut expected = Sha256::new();
        expected.update(b"x/message.md");
        expected.update([0u8]);
        expected.update(hex(Sha256::digest(b"one")).as_bytes());
        expected.update(b"\n");
        assert_eq!(a, hex(expected.finalize()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discard_removes_every_staged_file_and_the_staging_directory() {
        let dir = temp_dir("discard");
        let out = dir.join("out");
        let mut stager = Stager::begin(&out).expect("begin");
        stager.add(rel(&["a"]), b"1").unwrap();
        stager.add(rel(&["b"]), b"2").unwrap();
        assert_eq!(staging_names(&out).len(), 2);
        stager.discard();
        assert!(!out.join(STAGING_DIR).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_moves_files_deletes_identical_ones_and_writes_the_root_last() {
        let dir = temp_dir("commit");
        let out = dir.join("out");
        let root_dir = out.join("m");
        // The target already holds "keep" with the same content.
        std::fs::create_dir_all(root_dir.join("a")).unwrap();
        std::fs::write(root_dir.join("a").join("keep.txt"), b"same").unwrap();

        let mut stager = Stager::begin(&out).expect("begin");
        stager.add(rel(&["a", "keep.txt"]), b"same").unwrap();
        stager.add(rel(&["a", "new.txt"]), b"new").unwrap();
        stager.add(rel(&[FOLDER_JSON_NAME]), b"complete").unwrap();
        stager.finish();
        // Sorted: "a/keep.txt", "a/new.txt", then the root "folder.json" ("a" < "folder.json").
        let dispositions = [
            Disposition::Identical,
            Disposition::Create,
            Disposition::Create,
        ];
        let (written, unchanged) = stager
            .commit(&root_dir, b"incomplete", &dispositions, 2)
            .expect("commit");
        assert_eq!((written, unchanged), (2, 1));
        assert_eq!(
            std::fs::read(root_dir.join("a").join("new.txt")).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(root_dir.join("a").join("keep.txt")).unwrap(),
            b"same"
        );
        assert_eq!(
            std::fs::read(root_dir.join(FOLDER_JSON_NAME)).unwrap(),
            b"complete",
            "the complete root replaces the incomplete one"
        );
        assert!(!out.join(STAGING_DIR).exists(), "staging is emptied");
        assert_eq!(stager.read_back_mismatches(&root_dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_with_no_changes_touches_nothing_in_the_target() {
        let dir = temp_dir("nochange");
        let out = dir.join("out");
        let root_dir = out.join("m");
        std::fs::create_dir_all(&root_dir).unwrap();
        std::fs::write(root_dir.join(FOLDER_JSON_NAME), b"complete").unwrap();
        let mut stager = Stager::begin(&out).expect("begin");
        stager.add(rel(&[FOLDER_JSON_NAME]), b"complete").unwrap();
        stager.finish();
        let result = stager
            .commit(&root_dir, b"incomplete", &[Disposition::Identical], 0)
            .expect("commit");
        assert_eq!(result, (0, 1));
        assert_eq!(
            std::fs::read(root_dir.join(FOLDER_JSON_NAME)).unwrap(),
            b"complete"
        );
        assert!(!out.join(STAGING_DIR).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_back_reports_missing_and_changed_files() {
        let dir = temp_dir("readback");
        let out = dir.join("out");
        let root_dir = out.join("m");
        let mut stager = Stager::begin(&out).expect("begin");
        stager.add(rel(&["a.txt"]), b"alpha").unwrap();
        stager.add(rel(&["b.txt"]), b"bravo").unwrap();
        stager.add(rel(&[FOLDER_JSON_NAME]), b"root").unwrap();
        stager.finish();
        let dispositions = vec![Disposition::Create; 3];
        stager
            .commit(&root_dir, b"incomplete", &dispositions, 3)
            .expect("commit");
        assert_eq!(stager.read_back_mismatches(&root_dir), 0);
        std::fs::write(root_dir.join("a.txt"), b"ALPHA").unwrap();
        std::fs::remove_file(root_dir.join("b.txt")).unwrap();
        assert_eq!(stager.read_back_mismatches(&root_dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leftover_numeric_file_is_cleaned_and_a_foreign_entry_stops_the_run() {
        let dir = temp_dir("leftover");
        let out = dir.join("out");
        std::fs::create_dir_all(out.join(STAGING_DIR)).unwrap();
        std::fs::write(out.join(STAGING_DIR).join("7"), b"stale").unwrap();
        let stager = Stager::begin(&out).expect("a numeric leftover is removed");
        assert!(staging_names(&out).is_empty());
        stager.discard();

        std::fs::create_dir_all(out.join(STAGING_DIR)).unwrap();
        std::fs::write(out.join(STAGING_DIR).join("mine.txt"), b"x").unwrap();
        assert!(Stager::begin(&out).is_err());
        assert_eq!(
            std::fs::read(out.join(STAGING_DIR).join("mine.txt")).unwrap(),
            b"x"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hashing_a_large_file_in_chunks_gives_the_same_digest_as_hashing_it_whole() {
        let dir = temp_dir("chunks");
        let path = dir.join("big.bin");
        let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let (digest, size) = hash_file(&path).expect("hash");
        let mut whole = [0u8; 32];
        whole.copy_from_slice(&Sha256::digest(&content));
        assert_eq!(digest, whole);
        assert_eq!(size, 200_000);
        assert!(hash_file(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- add_stream (M4f) ---------------------------------------------------------------

    /// Returns at most `step` bytes per call, like a source that reads one sector at a time.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(self.data.len()).min(buf.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    /// Yields `good` bytes, then fails.
    struct Breaks {
        good: usize,
    }

    impl Read for Breaks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.good == 0 {
                return Err(std::io::Error::other("broken"));
            }
            let n = self.good.min(buf.len());
            buf[..n].fill(7);
            self.good -= n;
            Ok(n)
        }
    }

    /// Fails once with `Interrupted`, then reads normally.
    struct InterruptsOnce<'a> {
        data: &'a [u8],
        tripped: bool,
    }

    impl Read for InterruptsOnce<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.tripped {
                self.tripped = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.data.len().min(buf.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> [u8; 32] {
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&Sha256::digest(bytes));
        digest
    }

    #[test]
    fn streaming_stages_the_same_file_and_manifest_entry_as_add() {
        let dir = temp_dir("stream-equal");
        let payload = patterned(3 * COPY_CHUNK_BYTES + 17);

        let out_a = dir.join("a");
        let mut by_add = Stager::begin(&out_a).unwrap();
        by_add.add(rel(&["x", "f.bin"]), &payload).unwrap();
        by_add.finish();

        let out_b = dir.join("b");
        let mut by_stream = Stager::begin(&out_b).unwrap();
        let outcome = by_stream
            .add_stream(
                rel(&["x", "f.bin"]),
                &mut std::io::Cursor::new(&payload),
                Some(payload.len() as u64),
            )
            .unwrap();
        by_stream.finish();

        assert_eq!(
            outcome,
            StreamOutcome::Staged {
                size: payload.len() as u64,
                digest: sha(&payload)
            }
        );
        assert_eq!(by_add.files()[0].size, by_stream.files()[0].size);
        assert_eq!(by_add.files()[0].digest, by_stream.files()[0].digest);
        assert_eq!(by_add.tree_hash(), by_stream.tree_hash());
        assert_eq!(
            std::fs::read(out_a.join(STAGING_DIR).join("1")).unwrap(),
            std::fs::read(out_b.join(STAGING_DIR).join("1")).unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_reads_and_an_interrupt_do_not_change_the_result() {
        let dir = temp_dir("stream-short");
        let out = dir.join("out");
        let payload = patterned(5000);
        let mut stager = Stager::begin(&out).unwrap();
        let trickled = stager
            .add_stream(
                rel(&["a"]),
                &mut Trickle {
                    data: &payload,
                    step: 7,
                },
                None,
            )
            .unwrap();
        let interrupted = stager
            .add_stream(
                rel(&["b"]),
                &mut InterruptsOnce {
                    data: &payload,
                    tripped: false,
                },
                None,
            )
            .unwrap();
        let expected = StreamOutcome::Staged {
            size: 5000,
            digest: sha(&payload),
        };
        assert_eq!(trickled, expected);
        assert_eq!(interrupted, expected);
        assert_eq!(staging_names(&out), vec!["1", "2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_source_stages_an_empty_file() {
        let dir = temp_dir("stream-empty");
        let out = dir.join("out");
        let mut stager = Stager::begin(&out).unwrap();
        let outcome = stager
            .add_stream(rel(&["e"]), &mut std::io::empty(), Some(0))
            .unwrap();
        assert_eq!(
            outcome,
            StreamOutcome::Staged {
                size: 0,
                digest: sha(b"")
            }
        );
        assert_eq!(std::fs::read(out.join(STAGING_DIR).join("1")).unwrap(), b"");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_source_that_fails_leaves_nothing_behind_and_uses_no_number() {
        let dir = temp_dir("stream-fail");
        let out = dir.join("out");
        let mut stager = Stager::begin(&out).unwrap();

        let broken = stager
            .add_stream(rel(&["x"]), &mut Breaks { good: 1000 }, Some(5000))
            .unwrap();
        assert_eq!(
            broken,
            StreamOutcome::SourceFailed(SourceFailure::ReadError)
        );

        let payload = patterned(11);
        let wrong = stager
            .add_stream(rel(&["y"]), &mut std::io::Cursor::new(&payload), Some(10))
            .unwrap();
        assert_eq!(
            wrong,
            StreamOutcome::SourceFailed(SourceFailure::LengthMismatch)
        );

        assert!(stager.files().is_empty());
        assert!(staging_names(&out).is_empty());

        // The number was not used up: the next file is number 1.
        stager.add(rel(&["z"]), b"z").unwrap();
        assert_eq!(staging_names(&out), vec!["1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
