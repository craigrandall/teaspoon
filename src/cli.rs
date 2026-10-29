//! Command-line arguments and input classification.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use crate::shared::MAX_DIRECTORY_DEPTH;

// =============================================================================
// CLI and input classification
// =============================================================================

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe inventory and Markdown export for Outlook PST and MSG files (the teaspoon miner)"
)]
pub(crate) struct Args {
    /// A .pst file, a single .msg file, or a directory of .msg files to inspect.
    pub(crate) input: PathBuf,

    /// Compare the custom MS-OXMSG extraction path against `msg_parser`
    /// for the same .msg input. Reads real property content internally to
    /// do the comparison, but prints only match/mismatch counts -- never
    /// the values compared. Also runs the custom path's structural
    /// accounting (every CFB entry classified, every properties stream
    /// and value stream decoded) and prints its gate counters plus
    /// `structural_gate_violations` (0 on a clean corpus). When any gate
    /// is nonzero it also prints the privacy-safe structural breakdown
    /// needed to triage it. PST input is unaffected.
    #[arg(long)]
    pub(crate) verify: bool,

    /// Compare the envelope the export extracts (subject, sender, To/Cc/Bcc recipients) against
    /// `msg_parser` for the same .msg input, field by field. Reads real content internally and
    /// prints only match/mismatch counts, plus counts of which envelope fields the corpus
    /// carries -- never the values.
    #[arg(long, conflicts_with_all = ["verify", "out", "dry_run"])]
    pub(crate) verify_envelope: bool,

    /// Compare the HTML the in-house MS-OXRTFEX de-encapsulation recovers from each message's
    /// RTF against `msg_parser`'s `html_from_rtf()`, and the 10-token recognition rule against
    /// the whole-document `\fromhtml1` search. Reads real content internally and prints only
    /// counts, flags, code page numbers, and byte-size differences -- never the HTML.
    #[arg(long, conflicts_with_all = ["verify", "verify_envelope", "out", "dry_run"])]
    pub(crate) verify_deencap: bool,

    /// With `--verify-deencap`, write each message's two recovered HTML strings and its
    /// decompressed RTF into DIR (`NNN.custom.html`, `NNN.msg_parser.html`, `NNN.rtf`, where
    /// NNN is the message's position in the scanned list), so a pair can be compared locally.
    /// This is the only place message content goes; standard output stays content-free. DIR
    /// must be new or empty.
    #[arg(long, value_name = "DIR", requires = "verify_deencap")]
    pub(crate) dump_deencap: Option<PathBuf>,

    /// Content-preservation check (M4e-2c): for each message whose body is HTML (native, or
    /// recovered from the RTF), run the export's own preprocessing and conversion and compare,
    /// in order, the visible words of the HTML with the words of the Markdown. Reads real
    /// content internally and prints only counts (words in each, words matched in order, words
    /// missing, words extra, per-message difference counts) -- never the words.
    #[arg(
        long,
        conflicts_with_all = ["verify", "verify_envelope", "verify_deencap", "dump_deencap", "bakeoff_md", "bakeoff_dump", "out", "dry_run"]
    )]
    pub(crate) verify_bodies: bool,

    /// With `--verify-bodies`, write the two normalized word lists (one word per line) of every
    /// message that has a difference into DIR (`NNN.html.words.txt`, `NNN.markdown.words.txt`,
    /// where NNN is the message's position in the scanned list), so they can be compared
    /// locally with any diff tool. This is the only place message content goes; standard output
    /// stays content-free. DIR must be new or empty.
    #[arg(long, value_name = "DIR", requires = "verify_bodies")]
    pub(crate) dump_bodies: Option<PathBuf>,

    /// Attachment payload differential (M4f-2c): for each top-level by-value attachment, compare
    /// the SHA-256 of the bytes the export would write (read through the export's own reader)
    /// with the SHA-256 of `msg_parser`'s payload for the same attachment, pairing attachments
    /// by position in storage order. Reads real content internally and prints only counts
    /// (matches, mismatches, missing or unreadable streams, a count of positions where the two
    /// disagree on the method, and a gate total that must be 0) -- never names or bytes.
    #[arg(
        long,
        conflicts_with_all = ["verify", "verify_envelope", "verify_deencap", "dump_deencap", "verify_bodies", "dump_bodies", "bakeoff_md", "bakeoff_dump", "out", "dry_run"]
    )]
    pub(crate) verify_attachments: bool,

    /// Run the HTML-to-Markdown bake-off (M4e-0) over the `NNN.custom.html` files in the input
    /// directory (the directory `--dump-deencap` wrote) and a built-in set of synthetic and
    /// adversarial cases, and print only content-free scores. Needs a build with
    /// `--features bakeoff`, which compiles in the candidate converters.
    #[arg(
        long,
        conflicts_with_all = ["verify", "verify_envelope", "verify_deencap", "dump_deencap", "recursive", "out", "dry_run"]
    )]
    pub(crate) bakeoff_md: bool,

    /// With `--bakeoff-md`, write each candidate's Markdown for every input into DIR (the only
    /// place content goes). DIR must be new or empty.
    #[arg(long, value_name = "DIR", requires = "bakeoff_md")]
    pub(crate) bakeoff_dump: Option<PathBuf>,

    /// With `--verify-envelope`, `--verify-deencap`, `--verify-bodies`, or
    /// `--verify-attachments`, descend into subdirectories of a directory input (the default
    /// scan is non-recursive, as for `--verify`).
    #[arg(long)]
    pub(crate) recursive: bool,

    /// Directory to export into. The archive is written to `<DIR>/<input name>/`: one
    /// directory per message holding `message.md` and `metadata.json`, a `folder.json` in every
    /// folder, and a `folder.json` at the archive root that marks the directory as created
    /// by `tsp`. Exporting `.msg` input works; a `.pst` export arrives with M4i. Standard output
    /// stays content-free (counts only). With `--dry-run` nothing is written.
    #[arg(long, value_name = "DIR", conflicts_with = "verify")]
    pub(crate) out: Option<PathBuf>,

    /// Plan an export of the input into `--out` without writing anything,
    /// and print only content-free counts (the naming census): how many
    /// names need sanitizing or shortening, how many collide, how long the
    /// longest path is, and whether every planned path fits the Windows
    /// budget. A directory input is planned recursively, mirroring its
    /// subdirectories as folders.
    #[arg(long, requires = "out", conflicts_with = "verify")]
    pub(crate) dry_run: bool,

    /// Consent, without asking, to replace files that an earlier export by `tsp` generated
    /// and that this export would change. Without it, `tsp` asks (interactive terminal) or
    /// stops with exit code 2 (not interactive). It never touches files it did not generate and
    /// never writes into a directory it did not create.
    #[arg(long, requires = "out", conflicts_with = "dry_run")]
    pub(crate) overwrite: bool,

    /// Do not compute the SHA-256 of the source file(s) recorded in the archive root's
    /// `folder.json`.
    #[arg(long, requires = "out")]
    pub(crate) no_source_hash: bool,
}

pub(crate) enum InputKind {
    Pst,
    Msg {
        files: Vec<PathBuf>,
        /// Subdirectories found directly inside the scanned directory but
        /// not descended into (the scan is deliberately non-recursive).
        /// Surfaced in diagnostic output so this scoping choice is visible
        /// in the output, not just in the source.
        subdirectories_skipped: u64,
    },
}

fn is_msg_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("msg"))
        .unwrap_or(false)
}

/// Every `.msg` file under `root`, down to [`MAX_DIRECTORY_DEPTH`] levels, sorted, and the number
/// of directories below that depth that were not entered (guards against link loops; counted,
/// never silently skipped). Used by `--recursive`. Never includes the path in an error message.
pub(crate) fn collect_msg_files_recursive(root: &Path) -> Result<(Vec<PathBuf>, u64)> {
    let mut files = Vec::new();
    let mut beyond_depth_cap = 0u64;
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .context("failed to read input directory")?
            .filter_map(|entry| entry.ok())
        {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                if depth < MAX_DIRECTORY_DEPTH {
                    pending.push((entry_path, depth + 1));
                } else {
                    beyond_depth_cap += 1;
                }
            } else if is_msg_file(&entry_path) {
                files.push(entry_path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("input directory contains no .msg files");
    }
    Ok((files, beyond_depth_cap))
}

/// Classifies the input by extension (or, for a directory, by scanning for
/// `.msg` files directly inside it -- not recursive). Never includes the
/// input path itself in any error message: paths can disclose information
/// about the user or their mailbox.
pub(crate) fn classify_input(path: &Path) -> Result<InputKind> {
    if path.is_dir() {
        let mut files = Vec::new();
        let mut subdirectories_skipped = 0u64;

        for entry in std::fs::read_dir(path)
            .context("failed to read input directory")?
            .filter_map(|entry| entry.ok())
        {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                subdirectories_skipped += 1;
                continue;
            }
            if is_msg_file(&entry_path) {
                files.push(entry_path);
            }
        }
        files.sort();
        if files.is_empty() {
            anyhow::bail!("input directory contains no .msg files");
        }
        return Ok(InputKind::Msg {
            files,
            subdirectories_skipped,
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("pst") => Ok(InputKind::Pst),
        Some("msg") => Ok(InputKind::Msg {
            files: vec![path.to_path_buf()],
            subdirectories_skipped: 0,
        }),
        _ => anyhow::bail!(
            "unsupported input: expected a .pst file, a .msg file, or a directory of .msg files"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_recursive_scan_stops_at_the_depth_cap_and_counts_what_it_did_not_enter() {
        let root = temp_dir("deep");
        let mut dir = root.clone();
        // Two levels more than the cap: the first is seen and counted, the second never reached.
        for _ in 0..(MAX_DIRECTORY_DEPTH + 2) {
            dir.push("d");
        }
        std::fs::create_dir_all(&dir).expect("create the deep chain");
        std::fs::write(root.join("a.msg"), b"x").expect("write a file");

        let (files, beyond_depth_cap) = collect_msg_files_recursive(&root).expect("scan");
        assert_eq!(files.len(), 1);
        assert_eq!(beyond_depth_cap, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recursive_scan_finds_files_at_every_level_within_the_cap() {
        let root = temp_dir("levels");
        std::fs::create_dir_all(root.join("a").join("b")).expect("create directories");
        for path in [
            root.join("top.msg"),
            root.join("a").join("mid.MSG"),
            root.join("a").join("b").join("low.msg"),
            root.join("a").join("ignored.txt"),
        ] {
            std::fs::write(path, b"x").expect("write a file");
        }
        let (files, beyond_depth_cap) = collect_msg_files_recursive(&root).expect("scan");
        assert_eq!(files.len(), 3);
        assert_eq!(beyond_depth_cap, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
