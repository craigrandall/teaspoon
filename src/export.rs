//! Export writer (M4c walking skeleton): `tsp <input.msg|dir> --out <dir>`.
//!
//! Pipeline: read the source tree, plan names and paths (the same pure planner as `--dry-run`),
//! refuse if the plan breaks a gate, render every file in memory, compare with what is already
//! on disk (preflight, counts only), decide consent, write through a staging directory, read
//! everything back, and print content-free counts. Archive content goes only into the directory
//! the user named; stdout and stderr never carry names, subjects, or paths.
//!
//! Limits, stated plainly: only `.msg` input and the plain-text body are exported; attachments
//! and embedded messages are counted and recorded as not extracted (M4f); the whole archive is
//! rendered in memory before anything is written (fine for a skeleton, revisited in M4h); each
//! file is replaced atomically, but the archive as a whole is not all-or-nothing -- an
//! interrupted run leaves the root `folder.json` marked `incomplete`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::archive::{
    adjustment_names, message_metadata, render_message_md, to_json, ChildRecord, FolderMetadata,
    NameRecord, RootCounts, RootRecord, RootSpec, SourceRecord, FOLDER_JSON_NAME,
    KIND_ARCHIVE_ROOT, MESSAGE_MD_NAME, METADATA_JSON_NAME, STATUS_COMPLETE, STATUS_INCOMPLETE,
    TOOL_NAME,
};
use crate::dry_run::{classify_source, input_leaf_name, root_stem, root_units, SourceKind};
use crate::model::PlainBody;
use crate::naming::{collision_key, utf16_len};
use crate::plan::{plan_export, verify_plan, EntryKind, Plan, Policy, SourceFolder, SourceMessage};
use crate::source_msg::{build_msg_export_source, read_message_content, MsgFileMap};

/// Staging directory directly under `--out`. Files are written here under short numeric names
/// and then renamed into place, so a half-written file never appears at its final path.
const STAGING_DIR: &str = ".tsp-tmp";
/// `<out>\.tsp-tmp\<up to 10 digits>`: the longest staged path is `out + 1 + 8 + 1 + 10`.
const STAGING_TAIL_UNITS: usize = 1 + 8 + 1 + 10;
const MAX_PATH_UNITS: usize = 259;

#[derive(Debug, Clone)]
pub(crate) struct ExportOptions {
    /// `--overwrite`: consent to replace files this tool generated earlier.
    pub(crate) overwrite: bool,
    /// Record the SHA-256 of the source (the default; `--no-source-hash` turns it off).
    pub(crate) source_hash: bool,
    /// Standard input is a terminal, so the user can be asked.
    pub(crate) interactive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportStatus {
    Completed,
    /// Nothing was written (or, for `PlanOverBudget`, nothing was attempted). Exit code 2.
    Refused,
}

/// What the target directory `<out>/<stem>` is, before the export touches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ownership {
    /// Does not exist.
    Absent,
    /// Exists and holds nothing.
    Empty,
    /// Holds a root `folder.json` written by `tsp` for the same source.
    Owned,
    /// Holds a root `folder.json` written by `tsp` for a different source.
    OtherSource,
    /// Exists, is not empty, and `tsp` did not create it (or it is not a directory).
    NotOwned,
}

impl Ownership {
    fn label(self) -> &'static str {
        match self {
            Ownership::Absent => "absent",
            Ownership::Empty => "empty",
            Ownership::Owned => "owned",
            Ownership::OtherSource => "other_source",
            Ownership::NotOwned => "not_owned",
        }
    }
}

/// Counts only: how the planned files compare with what is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preflight {
    pub(crate) ownership: Ownership,
    pub(crate) to_create: usize,
    pub(crate) identical: usize,
    pub(crate) to_replace: usize,
    /// Planned paths that something unreadable or a directory occupies.
    pub(crate) blocked: usize,
    /// Entries in the target that this export would not touch.
    pub(crate) unrelated_present: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefuseReason {
    NotOwned,
    SourceMismatch,
    Blocked,
    NeedsConsent,
    PlanOverBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Proceed,
    Ask,
    Refuse(RefuseReason),
}

/// Consent rules (pure). Never overwrites what `tsp` did not create; replaces its own files only
/// with consent (`--overwrite`, or a yes at the prompt).
pub(crate) fn decide(p: &Preflight, overwrite: bool, interactive: bool) -> Decision {
    match p.ownership {
        Ownership::NotOwned => return Decision::Refuse(RefuseReason::NotOwned),
        Ownership::OtherSource => return Decision::Refuse(RefuseReason::SourceMismatch),
        Ownership::Absent | Ownership::Empty | Ownership::Owned => {}
    }
    if p.blocked > 0 {
        return Decision::Refuse(RefuseReason::Blocked);
    }
    if p.to_replace == 0 || overwrite {
        Decision::Proceed
    } else if interactive {
        Decision::Ask
    } else {
        Decision::Refuse(RefuseReason::NeedsConsent)
    }
}

fn refusal_label(reason: RefuseReason) -> &'static str {
    match reason {
        RefuseReason::NotOwned => "refused_target_not_owned",
        RefuseReason::SourceMismatch => "refused_target_source_mismatch",
        RefuseReason::Blocked => "refused_target_blocked",
        RefuseReason::NeedsConsent => "refused_needs_consent",
        RefuseReason::PlanOverBudget => "refused_plan_over_budget",
    }
}

fn refusal_message(reason: RefuseReason, p: &Preflight) -> String {
    match reason {
        RefuseReason::NotOwned => "the target directory already exists and was not created by tsp; nothing was written".to_string(),
        RefuseReason::SourceMismatch => "the target directory holds an export of a different source; nothing was written".to_string(),
        RefuseReason::Blocked => format!(
            "{} planned file(s) cannot be written because something else occupies their place; nothing was written",
            p.blocked
        ),
        RefuseReason::NeedsConsent => format!(
            "{} existing file(s) would be replaced; run interactively or add --overwrite; nothing was written",
            p.to_replace
        ),
        RefuseReason::PlanOverBudget => "some planned paths exceed the Windows path budget; choose a shorter --out directory; nothing was written".to_string(),
    }
}

/// One file the export will write, relative to the archive root.
#[derive(Debug, Clone)]
struct Generated {
    rel: Vec<String>,
    content: String,
}

/// Content-free results printed at the end (all keys always printed, zero when not applicable).
#[derive(Debug, Clone, Default)]
pub(crate) struct ExportReport {
    root_units: usize,
    plan_entries_total: usize,
    plan_folders: usize,
    plan_messages: usize,
    plan_attachment_files: usize,
    plan_embedded_messages: usize,
    plan_budget_exceeded: usize,
    plan_gate_violations: usize,
    plan_max_relative_path_units: usize,
    target_state: &'static str,
    pre_to_create: usize,
    pre_identical: usize,
    pre_to_replace: usize,
    pre_blocked: usize,
    pre_unrelated_present: usize,
    consent: &'static str,
    result: &'static str,
    files_written: usize,
    files_unchanged: usize,
    folder_files: usize,
    message_directories: usize,
    attachments_not_extracted: usize,
    embedded_messages_not_extracted: usize,
    bodies_plain_present: usize,
    bodies_plain_empty: usize,
    bodies_plain_absent: usize,
    readback_mismatches: usize,
    tree_sha256: String,
}

fn print_export_report(r: &ExportReport) {
    println!("inventory=privacy_safe");
    println!("input_kind=msg_export");
    println!("export_root_units={}", r.root_units);
    println!("plan_entries_total={}", r.plan_entries_total);
    println!("plan_folders={}", r.plan_folders);
    println!("plan_messages={}", r.plan_messages);
    println!("plan_attachment_files={}", r.plan_attachment_files);
    println!("plan_embedded_messages={}", r.plan_embedded_messages);
    println!("plan_budget_exceeded={}", r.plan_budget_exceeded);
    println!("plan_gate_violations={}", r.plan_gate_violations);
    println!(
        "plan_max_relative_path_units={}",
        r.plan_max_relative_path_units
    );
    println!("export_target_state={}", r.target_state);
    println!("export_preflight_to_create={}", r.pre_to_create);
    println!("export_preflight_identical={}", r.pre_identical);
    println!("export_preflight_to_replace={}", r.pre_to_replace);
    println!("export_preflight_blocked={}", r.pre_blocked);
    println!(
        "export_preflight_unrelated_present={}",
        r.pre_unrelated_present
    );
    println!("export_consent={}", r.consent);
    println!("export_result={}", r.result);
    println!("export_files_written={}", r.files_written);
    println!("export_files_unchanged={}", r.files_unchanged);
    println!("export_folder_files={}", r.folder_files);
    println!("export_message_directories={}", r.message_directories);
    println!(
        "export_attachments_not_extracted={}",
        r.attachments_not_extracted
    );
    println!(
        "export_embedded_messages_not_extracted={}",
        r.embedded_messages_not_extracted
    );
    println!("export_bodies_plain_present={}", r.bodies_plain_present);
    println!("export_bodies_plain_empty={}", r.bodies_plain_empty);
    println!("export_bodies_plain_absent={}", r.bodies_plain_absent);
    println!("export_readback_mismatches={}", r.readback_mismatches);
    println!("export_tree_sha256={}", r.tree_sha256);
}

// -----------------------------------------------------------------------------
// Small helpers
// -----------------------------------------------------------------------------

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = std::fs::File::open(path).context("failed to open a source file for hashing")?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .context("failed to read a source file for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex(hasher.finalize()), total))
}

/// The source file's name (single file) or its path relative to the input directory with `/`
/// separators. Never absolute.
fn relative_source_name(input: &Path, file: &Path) -> String {
    if input.is_dir() {
        if let Ok(rel) = file.strip_prefix(input) {
            return rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
        }
    }
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn join_path(root: &Path, rel: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in rel {
        path.push(part);
    }
    path
}

fn rel_with(components: &[String], leaf: &str) -> Vec<String> {
    let mut rel = components.to_vec();
    rel.push(leaf.to_string());
    rel
}

fn source_record(input: &Path, files: &MsgFileMap, hash: bool) -> Result<SourceRecord> {
    let name = input_leaf_name(input);
    if !input.is_dir() {
        let size = std::fs::metadata(input)
            .context("failed to read the source file")?
            .len();
        let sha256 = if hash {
            Some(sha256_file(input)?.0)
        } else {
            None
        };
        return Ok(SourceRecord {
            kind: "msg_file",
            name,
            size_bytes: size,
            sha256,
            sha256_scope: "file",
        });
    }
    let mut entries: Vec<(String, &PathBuf)> = files
        .values()
        .map(|p| (relative_source_name(input, p), p))
        .collect();
    entries.sort();
    let mut total = 0u64;
    let mut listing = Sha256::new();
    for (rel, path) in &entries {
        if hash {
            let (digest, size) = sha256_file(path)?;
            total += size;
            listing.update(rel.as_bytes());
            listing.update(b"\t");
            listing.update(digest.as_bytes());
            listing.update(b"\n");
        } else {
            total += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(SourceRecord {
        kind: "msg_directory",
        name,
        size_bytes: total,
        sha256: if hash {
            Some(hex(listing.finalize()))
        } else {
            None
        },
        sha256_scope: "listing_of_exported_msg_files",
    })
}

fn index_tree<'a>(
    folder: &'a SourceFolder,
    folders: &mut BTreeMap<u64, &'a SourceFolder>,
    messages: &mut BTreeMap<u64, &'a SourceMessage>,
) {
    folders.insert(folder.id, folder);
    for message in &folder.messages {
        messages.insert(message.id, message);
    }
    for sub in &folder.folders {
        index_tree(sub, folders, messages);
    }
}

// -----------------------------------------------------------------------------
// Rendering the archive in memory
// -----------------------------------------------------------------------------

struct Rendered {
    /// Every file, including the root `folder.json` (complete status), sorted by path.
    files: Vec<Generated>,
    root: RootSpec,
}

fn render_archive(
    input: &Path,
    tree: &SourceFolder,
    plan: &Plan,
    policy: &Policy,
    sources: &MsgFileMap,
    source: SourceRecord,
    report: &mut ExportReport,
) -> Result<Rendered> {
    let mut folders = BTreeMap::new();
    let mut messages = BTreeMap::new();
    index_tree(tree, &mut folders, &mut messages);

    let mut children: BTreeMap<Vec<String>, Vec<ChildRecord>> = BTreeMap::new();
    for entry in &plan.entries {
        let kind = match entry.kind {
            EntryKind::Folder => "folder",
            EntryKind::Message => "message",
            EntryKind::AttachmentFile | EntryKind::EmbeddedMessage => continue,
        };
        if let Some((last, parent)) = entry.components.split_last() {
            children
                .entry(parent.to_vec())
                .or_default()
                .push(ChildRecord {
                    kind,
                    directory_name: last.clone(),
                });
        }
    }

    let mut files: Vec<Generated> = Vec::new();
    for entry in &plan.entries {
        match entry.kind {
            EntryKind::Folder => {
                if !folders.contains_key(&entry.source_id) {
                    bail!("internal error: a planned folder has no source folder");
                }
                let meta = FolderMetadata::folder(
                    NameRecord {
                        original: entry.original.clone(),
                        adjustments: adjustment_names(&entry.flags),
                    },
                    children.remove(&entry.components).unwrap_or_default(),
                );
                files.push(Generated {
                    rel: rel_with(&entry.components, FOLDER_JSON_NAME),
                    content: to_json(&meta)?,
                });
                report.folder_files += 1;
            }
            EntryKind::Message => {
                let message = messages
                    .get(&entry.source_id)
                    .ok_or_else(|| anyhow!("internal error: a planned message has no source"))?;
                let path = sources
                    .get(&entry.source_id)
                    .ok_or_else(|| anyhow!("internal error: a planned message has no file"))?;
                let content = read_message_content(path)?;
                let meta = message_metadata(
                    &relative_source_name(input, path),
                    &content,
                    NameRecord {
                        original: entry.original.clone(),
                        adjustments: adjustment_names(&entry.flags),
                    },
                    message.attachments.len(),
                );
                files.push(Generated {
                    rel: rel_with(&entry.components, MESSAGE_MD_NAME),
                    content: render_message_md(&content),
                });
                files.push(Generated {
                    rel: rel_with(&entry.components, METADATA_JSON_NAME),
                    content: to_json(&meta)?,
                });
                report.message_directories += 1;
                match content.plain_body {
                    PlainBody::Text(_) => report.bodies_plain_present += 1,
                    PlainBody::Empty => report.bodies_plain_empty += 1,
                    PlainBody::Absent => report.bodies_plain_absent += 1,
                }
            }
            EntryKind::AttachmentFile => report.attachments_not_extracted += 1,
            EntryKind::EmbeddedMessage => report.embedded_messages_not_extracted += 1,
        }
    }

    let root = RootSpec {
        record: RootRecord {
            tool: TOOL_NAME,
            tool_version: env!("CARGO_PKG_VERSION"),
            status: STATUS_COMPLETE,
            source,
            max_path_units_allowed: policy.max_path_units,
            planned_longest_relative_path_units: plan
                .counters
                .max_path_units
                .saturating_sub(policy.root_units),
            counts: RootCounts {
                folders: plan.counters.folders,
                messages: plan.counters.messages,
                attachments_not_extracted: plan.counters.attachment_files,
                embedded_messages_not_extracted: plan.counters.embedded_messages,
            },
        },
        children: children.remove(&Vec::new()).unwrap_or_default(),
    };
    files.push(Generated {
        rel: vec![FOLDER_JSON_NAME.to_string()],
        content: root.render(STATUS_COMPLETE)?,
    });
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    report.folder_files += 1;
    Ok(Rendered { files, root })
}

fn is_root_file(file: &Generated) -> bool {
    file.rel.len() == 1 && file.rel[0] == FOLDER_JSON_NAME
}

/// One hash over all relative paths and file hashes, so two exports can be compared by a single
/// line of output.
fn tree_hash(files: &[Generated]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.rel.join("/").as_bytes());
        hasher.update([0u8]);
        hasher.update(hex(Sha256::digest(file.content.as_bytes())).as_bytes());
        hasher.update(b"\n");
    }
    hex(hasher.finalize())
}

// -----------------------------------------------------------------------------
// Preflight against the target directory
// -----------------------------------------------------------------------------

fn classify_target(root_dir: &Path, source: &SourceRecord) -> Ownership {
    let meta = match std::fs::symlink_metadata(root_dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ownership::Absent,
        Err(_) => return Ownership::NotOwned,
    };
    if !meta.is_dir() {
        return Ownership::NotOwned;
    }
    match std::fs::read_to_string(root_dir.join(FOLDER_JSON_NAME)) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) if is_root_marker(&value) => {
                if same_source(&value, source) {
                    Ownership::Owned
                } else {
                    Ownership::OtherSource
                }
            }
            _ => Ownership::NotOwned,
        },
        Err(_) => match std::fs::read_dir(root_dir) {
            Ok(mut entries) => {
                if entries.next().is_none() {
                    Ownership::Empty
                } else {
                    Ownership::NotOwned
                }
            }
            Err(_) => Ownership::NotOwned,
        },
    }
}

fn is_root_marker(value: &serde_json::Value) -> bool {
    value.get("kind").and_then(|v| v.as_str()) == Some(KIND_ARCHIVE_ROOT)
        && value.pointer("/root/tool").and_then(|v| v.as_str()) == Some(TOOL_NAME)
}

/// Same source means the same kind and name. The hash is deliberately not compared: re-exporting
/// a source that has changed since the last export is the ordinary reason to overwrite.
fn same_source(value: &serde_json::Value, source: &SourceRecord) -> bool {
    value.pointer("/root/source/kind").and_then(|v| v.as_str()) == Some(source.kind)
        && value.pointer("/root/source/name").and_then(|v| v.as_str()) == Some(source.name.as_str())
}

fn key_of(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| collision_key(p).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn count_unrelated(root_dir: &Path, files: &[Generated]) -> usize {
    let mut known: BTreeSet<String> = BTreeSet::new();
    for file in files {
        for i in 1..=file.rel.len() {
            known.insert(key_of(&file.rel[..i]));
        }
    }
    let mut unrelated = 0usize;
    let mut stack: Vec<(PathBuf, Vec<String>)> = vec![(root_dir.to_path_buf(), Vec::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let mut child = rel.clone();
            child.push(entry.file_name().to_string_lossy().into_owned());
            if !known.contains(&key_of(&child)) {
                unrelated += 1;
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push((entry.path(), child));
            }
        }
    }
    unrelated
}

fn preflight(root_dir: &Path, files: &[Generated], source: &SourceRecord) -> Preflight {
    let ownership = classify_target(root_dir, source);
    let mut p = Preflight {
        ownership,
        to_create: 0,
        identical: 0,
        to_replace: 0,
        blocked: 0,
        unrelated_present: 0,
    };
    for file in files {
        match std::fs::read(join_path(root_dir, &file.rel)) {
            Ok(existing) => {
                if existing == file.content.as_bytes() {
                    p.identical += 1;
                } else {
                    p.to_replace += 1;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => p.to_create += 1,
            Err(_) => p.blocked += 1,
        }
    }
    if ownership == Ownership::Owned {
        p.unrelated_present = count_unrelated(root_dir, files);
    }
    p
}

/// Asks on standard error (standard output stays counts-only). Only an explicit yes continues.
pub(crate) fn prompt_for_consent(p: &Preflight) -> bool {
    eprintln!(
        "This export would replace {} file(s) generated by an earlier export ({} would be created, {} already match, {} unrelated entries are left alone).",
        p.to_replace, p.to_create, p.identical, p.unrelated_present
    );
    eprint!("Continue? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

// -----------------------------------------------------------------------------
// Writing
// -----------------------------------------------------------------------------

fn prepare_staging(staging: &Path) -> Result<()> {
    match std::fs::read_dir(staging) {
        Ok(entries) => {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
                let leftover =
                    is_file && !name.is_empty() && name.chars().all(|c| c.is_ascii_digit());
                if !leftover {
                    bail!("the staging directory .tsp-tmp already holds entries this tool did not create; remove it and retry");
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

fn put_file(staging: &Path, counter: &mut u64, final_path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent).context("failed to create an output directory")?;
    }
    *counter += 1;
    let staged = staging.join(counter.to_string());
    std::fs::write(&staged, content.as_bytes()).context("failed to write a staged file")?;
    // On Windows `rename` replaces an existing file (MoveFileExW with MOVEFILE_REPLACE_EXISTING).
    std::fs::rename(&staged, final_path).context("failed to move a staged file into place")?;
    Ok(())
}

/// Writes only what differs. The root `folder.json` is written as `incomplete` first and
/// replaced with the complete version last, so an interrupted run is recognizable. Returns
/// (files written, files left unchanged).
fn write_archive(
    out: &Path,
    root_dir: &Path,
    rendered: &Rendered,
    pre: &Preflight,
) -> Result<(usize, usize)> {
    if pre.to_create + pre.to_replace == 0 {
        return Ok((0, rendered.files.len()));
    }
    std::fs::create_dir_all(out).context("failed to create the output directory")?;
    let staging = out.join(STAGING_DIR);
    prepare_staging(&staging)?;
    let mut counter = 0u64;

    let root_path = root_dir.join(FOLDER_JSON_NAME);
    put_file(
        &staging,
        &mut counter,
        &root_path,
        &rendered.root.render(STATUS_INCOMPLETE)?,
    )?;

    let mut written = 0usize;
    let mut unchanged = 0usize;
    let mut root_file: Option<&Generated> = None;
    for file in &rendered.files {
        if is_root_file(file) {
            root_file = Some(file);
            continue;
        }
        let path = join_path(root_dir, &file.rel);
        let same = std::fs::read(&path)
            .map(|existing| existing == file.content.as_bytes())
            .unwrap_or(false);
        if same {
            unchanged += 1;
        } else {
            put_file(&staging, &mut counter, &path, &file.content)?;
            written += 1;
        }
    }
    if let Some(file) = root_file {
        put_file(&staging, &mut counter, &root_path, &file.content)?;
        written += 1;
    }
    // Best effort: the directory is empty now; a failure here leaves only an empty folder.
    let _ = std::fs::remove_dir(&staging);
    Ok((written, unchanged))
}

fn read_back_mismatches(root_dir: &Path, files: &[Generated]) -> usize {
    files
        .iter()
        .filter(|file| {
            std::fs::read(join_path(root_dir, &file.rel))
                .map(|bytes| bytes != file.content.as_bytes())
                .unwrap_or(true)
        })
        .count()
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

/// Exports `.msg` input into `<out>/<input name>/`. `confirm` is called only when the decision is
/// to ask (interactive, files to replace, no `--overwrite`).
pub(crate) fn run_export(
    input: &Path,
    out: &Path,
    options: &ExportOptions,
    confirm: &mut dyn FnMut(&Preflight) -> bool,
) -> Result<ExportStatus> {
    match classify_source(input)? {
        SourceKind::Pst => bail!(
            "exporting a .pst file is not implemented yet (planned for M4i); add --dry-run to plan one"
        ),
        SourceKind::Msg => {}
    }

    let stem = root_stem(input);
    let absolute_out = std::path::absolute(out).context("failed to resolve the output path")?;
    if utf16_len(&absolute_out.to_string_lossy()) + STAGING_TAIL_UNITS > MAX_PATH_UNITS {
        bail!("the --out path is too long to stage files under it; choose a shorter directory");
    }
    let policy = Policy::new(root_units(out, &stem)?);
    let (tree, census, sources) = build_msg_export_source(input)?;
    if !input.is_dir() && census.open_errors > 0 {
        bail!("the input could not be opened as a .msg file");
    }

    let plan = plan_export(&tree, &policy);
    let gate = verify_plan(&plan, &policy);
    let mut report = ExportReport {
        root_units: policy.root_units,
        plan_entries_total: plan.counters.entries_total,
        plan_folders: plan.counters.folders,
        plan_messages: plan.counters.messages,
        plan_attachment_files: plan.counters.attachment_files,
        plan_embedded_messages: plan.counters.embedded_messages,
        plan_budget_exceeded: plan.counters.budget_exceeded,
        plan_gate_violations: gate.violations(),
        plan_max_relative_path_units: plan
            .counters
            .max_path_units
            .saturating_sub(policy.root_units),
        target_state: "not_checked",
        consent: "not_applicable",
        result: "completed",
        tree_sha256: "none".to_string(),
        ..ExportReport::default()
    };

    if gate.violations() > 0 || plan.counters.budget_exceeded > 0 {
        report.result = refusal_label(RefuseReason::PlanOverBudget);
        print_export_report(&report);
        let none = Preflight {
            ownership: Ownership::Absent,
            to_create: 0,
            identical: 0,
            to_replace: 0,
            blocked: 0,
            unrelated_present: 0,
        };
        eprintln!("{}", refusal_message(RefuseReason::PlanOverBudget, &none));
        return Ok(ExportStatus::Refused);
    }

    let source = source_record(input, &sources, options.source_hash)?;
    let rendered = render_archive(
        input,
        &tree,
        &plan,
        &policy,
        &sources,
        source.clone(),
        &mut report,
    )?;
    report.tree_sha256 = tree_hash(&rendered.files);

    let root_dir = out.join(&stem);
    let pre = preflight(&root_dir, &rendered.files, &source);
    report.target_state = pre.ownership.label();
    report.pre_to_create = pre.to_create;
    report.pre_identical = pre.identical;
    report.pre_to_replace = pre.to_replace;
    report.pre_blocked = pre.blocked;
    report.pre_unrelated_present = pre.unrelated_present;

    let proceed = match decide(&pre, options.overwrite, options.interactive) {
        Decision::Proceed => {
            report.consent = if pre.to_replace > 0 {
                "overwrite_flag"
            } else {
                "not_needed"
            };
            true
        }
        Decision::Ask => {
            if confirm(&pre) {
                report.consent = "prompted_accepted";
                true
            } else {
                report.consent = "prompted_declined";
                report.result = "refused_declined";
                false
            }
        }
        Decision::Refuse(reason) => {
            if reason == RefuseReason::NeedsConsent {
                report.consent = "non_interactive_declined";
            }
            report.result = refusal_label(reason);
            eprintln!("{}", refusal_message(reason, &pre));
            false
        }
    };
    if !proceed {
        print_export_report(&report);
        return Ok(ExportStatus::Refused);
    }

    let (written, unchanged) = write_archive(out, &root_dir, &rendered, &pre)?;
    report.files_written = written;
    report.files_unchanged = unchanged;
    report.readback_mismatches = read_back_mismatches(&root_dir, &rendered.files);
    print_export_report(&report);
    if report.readback_mismatches > 0 {
        bail!("read-back found files that differ from what was planned; the archive is not trustworthy");
    }
    Ok(ExportStatus::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use clap::Parser;

    // ---- synthetic input ----------------------------------------------------

    fn entry(ty: u16, id: u16, value: [u8; 8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&value);
        v
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    /// A plain-text message: subject, optional body, delivery time, and optionally one by-value
    /// attachment.
    fn write_msg(path: &Path, subject: &str, body: Option<&str>, with_attachment: bool) {
        let mut comp = cfb::create(path).expect("create CFB");
        let ticks: u64 = 133_000_000_000_000_000;
        let mut props = vec![0u8; 32];
        props.extend(entry(0x0040, 0x0E06, ticks.to_le_bytes()));
        put(&mut comp, "/__properties_version1.0", &props);
        put(&mut comp, "/__substg1.0_0037001F", &utf16(subject));
        if let Some(text) = body {
            put(&mut comp, "/__substg1.0_1000001F", &utf16(text));
        }
        if with_attachment {
            comp.create_storage("/__attach_version1.0_#00000000")
                .expect("create attachment storage");
            let mut a0 = vec![0u8; 8];
            a0.extend(entry(0x0003, 0x3705, [1, 0, 0, 0, 0, 0, 0, 0]));
            put(
                &mut comp,
                "/__attach_version1.0_#00000000/__properties_version1.0",
                &a0,
            );
            put(
                &mut comp,
                "/__attach_version1.0_#00000000/__substg1.0_3707001F",
                &utf16("report.pdf"),
            );
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-exp-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn opts(overwrite: bool, interactive: bool) -> ExportOptions {
        ExportOptions {
            overwrite,
            source_hash: true,
            interactive,
        }
    }

    fn never_asked(_: &Preflight) -> bool {
        panic!("the user must not be asked in this case");
    }

    fn export(input: &Path, out: &Path, options: &ExportOptions) -> ExportStatus {
        run_export(input, out, options, &mut never_asked).expect("export runs")
    }

    /// Every file under `root` as (relative path with `/`, bytes), sorted.
    fn tree_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, prefix: &str, into: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir)
                .expect("read dir")
                .filter_map(|e| e.ok())
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if entry.path().is_dir() {
                    walk(&entry.path(), &rel, into);
                } else {
                    into.insert(rel, std::fs::read(entry.path()).expect("read file"));
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(root, "", &mut files);
        files
    }

    fn lf(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    fn hello(dir: &Path) -> PathBuf {
        let file = dir.join("m.msg");
        write_msg(
            &file,
            "Hello World",
            Some("Line one\r\nLine two\r\n"),
            false,
        );
        file
    }

    // ---- the walking skeleton end to end -----------------------------------

    #[test]
    fn a_plain_text_message_exports_to_the_documented_layout() {
        let dir = temp_dir("layout");
        let input = hello(&dir);
        let out = dir.join("out");

        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec![
                "m/Hello World/message.md",
                "m/Hello World/metadata.json",
                "m/folder.json"
            ]
        );
        assert_eq!(
            lf(&String::from_utf8(files["m/Hello World/message.md"].clone()).unwrap()),
            lf(include_str!("../tests/golden/hello/message.md"))
        );
        assert_eq!(
            lf(&String::from_utf8(files["m/Hello World/metadata.json"].clone()).unwrap()),
            lf(include_str!("../tests/golden/hello/metadata.json"))
        );
        assert!(
            !out.join(STAGING_DIR).exists(),
            "staging directory is removed"
        );

        let root: serde_json::Value =
            serde_json::from_slice(&files["m/folder.json"]).expect("root is JSON");
        assert_eq!(root["kind"], "archive_root");
        assert_eq!(root["root"]["status"], "complete");
        assert_eq!(root["root"]["tool"], "teaspoon");
        assert_eq!(root["root"]["source"]["kind"], "msg_file");
        assert_eq!(root["root"]["source"]["name"], "m.msg");
        assert_eq!(root["root"]["source"]["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(root["root"]["counts"]["messages"], 1);
        assert_eq!(root["children"][0]["kind"], "message");
        assert_eq!(root["children"][0]["directory_name"], "Hello World");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_exports_of_the_same_input_are_byte_identical() {
        let dir = temp_dir("twice");
        let input = hello(&dir);
        let out_a = dir.join("a");
        let out_b = dir.join("b");
        assert_eq!(
            export(&input, &out_a, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(
            export(&input, &out_b, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out_a), tree_files(&out_b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_running_an_unchanged_export_writes_nothing_and_needs_no_consent() {
        let dir = temp_dir("rerun");
        let input = hello(&dir);
        let out = dir.join("out");
        export(&input, &out, &opts(false, false));
        let before = tree_files(&out);
        // Not interactive, no --overwrite: still fine, because nothing would change.
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_input_mirrors_subdirectories_and_numbers_duplicate_subjects() {
        let dir = temp_dir("dirinput");
        let src = dir.join("mail");
        std::fs::create_dir_all(src.join("Projects")).unwrap();
        write_msg(&src.join("a.msg"), "Status", Some("one"), false);
        write_msg(&src.join("b.msg"), "Status", Some("two"), false);
        write_msg(
            &src.join("Projects").join("c.msg"),
            "Status",
            Some("three"),
            false,
        );
        let out = dir.join("out");

        assert_eq!(
            export(&src, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        let files = tree_files(&out);
        for expected in [
            "mail/folder.json",
            "mail/Status/message.md",
            "mail/Status (02)/message.md",
            "mail/Projects/folder.json",
            "mail/Projects/Status/metadata.json",
        ] {
            assert!(files.contains_key(expected), "missing {expected}");
        }
        let root: serde_json::Value = serde_json::from_slice(&files["mail/folder.json"]).unwrap();
        assert_eq!(root["root"]["source"]["kind"], "msg_directory");
        assert_eq!(root["root"]["source"]["name"], "mail");
        assert_eq!(root["root"]["counts"]["folders"], 1);
        assert_eq!(root["root"]["counts"]["messages"], 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trailing_separator_on_a_directory_input_does_not_rename_the_archive() {
        let dir = temp_dir("trailing");
        let src = dir.join("mail");
        std::fs::create_dir_all(&src).unwrap();
        write_msg(&src.join("a.msg"), "Hi", Some("x"), false);
        let with_slash = PathBuf::from(format!("{}{}", src.display(), std::path::MAIN_SEPARATOR));
        let out = dir.join("out");
        assert_eq!(
            export(&with_slash, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert!(out.join("mail").join("Hi").join("message.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unusable_characters_in_a_subject_are_replaced_and_recorded() {
        let dir = temp_dir("sanitize");
        let file = dir.join("s.msg");
        write_msg(&file, "Re: a/b?", Some("x"), false);
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        let meta_key = files
            .keys()
            .find(|k| k.ends_with("/metadata.json"))
            .expect("one message directory")
            .clone();
        assert!(!meta_key.contains(':') && !meta_key.contains('?'));
        let meta: serde_json::Value = serde_json::from_slice(&files[&meta_key]).unwrap();
        assert_eq!(meta["directory"]["original"], "Re: a/b?");
        assert_eq!(meta["subject"], "Re: a/b?");
        assert!(!meta["directory"]["adjustments"]
            .as_array()
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attachments_are_counted_and_flagged_never_silently_dropped() {
        let dir = temp_dir("attach");
        let file = dir.join("a.msg");
        write_msg(&file, "With file", Some("see attached"), true);
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        assert!(
            !files.keys().any(|k| k.contains("attachments")),
            "M4c writes no attachments"
        );
        let meta: serde_json::Value =
            serde_json::from_slice(&files["a/With file/metadata.json"]).unwrap();
        assert_eq!(meta["attachments_not_extracted"], 1);
        assert_eq!(meta["status"], "partial");
        let reasons: Vec<&str> = meta["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(reasons.contains(&"attachments_not_extracted"));
        let root: serde_json::Value = serde_json::from_slice(&files["a/folder.json"]).unwrap();
        assert_eq!(root["root"]["counts"]["attachments_not_extracted"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_without_a_plain_body_still_gets_a_directory() {
        let dir = temp_dir("nobody");
        let file = dir.join("n.msg");
        write_msg(&file, "Empty one", None, false);
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        let files = tree_files(&out);
        assert_eq!(
            lf(&String::from_utf8(files["n/Empty one/message.md"].clone()).unwrap()),
            "# Empty one\n"
        );
        let meta: serde_json::Value =
            serde_json::from_slice(&files["n/Empty one/metadata.json"]).unwrap();
        assert_eq!(meta["body"]["plain_text"], "absent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the overwrite matrix ----------------------------------------------

    #[test]
    fn changed_generated_files_need_consent() {
        let dir = temp_dir("consent");
        let input = hello(&dir);
        let out = dir.join("out");
        export(&input, &out, &opts(false, false));
        let target = out.join("m").join("Hello World").join("message.md");
        let good = std::fs::read(&target).unwrap();

        // Not interactive, no --overwrite: refused, file untouched.
        std::fs::write(&target, b"edited by hand").unwrap();
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Refused
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"edited by hand");

        // Interactive and the user says no: refused, file untouched.
        let mut asked = 0;
        let status = run_export(&input, &out, &opts(false, true), &mut |p| {
            asked += 1;
            assert_eq!(p.to_replace, 1);
            false
        })
        .unwrap();
        assert_eq!(status, ExportStatus::Refused);
        assert_eq!(asked, 1);
        assert_eq!(std::fs::read(&target).unwrap(), b"edited by hand");

        // Interactive and the user says yes: replaced.
        let status = run_export(&input, &out, &opts(false, true), &mut |_| true).unwrap();
        assert_eq!(status, ExportStatus::Completed);
        assert_eq!(std::fs::read(&target).unwrap(), good);

        // --overwrite: replaced without asking.
        std::fs::write(&target, b"edited again").unwrap();
        assert_eq!(
            export(&input, &out, &opts(true, false)),
            ExportStatus::Completed
        );
        assert_eq!(std::fs::read(&target).unwrap(), good);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrelated_files_in_an_owned_archive_are_left_alone() {
        let dir = temp_dir("unrelated");
        let input = hello(&dir);
        let out = dir.join("out");
        export(&input, &out, &opts(false, false));
        std::fs::write(out.join("m").join("notes.txt"), b"mine").unwrap();
        std::fs::write(
            out.join("m").join("Hello World").join("extra.txt"),
            b"also mine",
        )
        .unwrap();

        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(
            std::fs::read(out.join("m").join("notes.txt")).unwrap(),
            b"mine"
        );
        assert_eq!(
            std::fs::read(out.join("m").join("Hello World").join("extra.txt")).unwrap(),
            b"also mine"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_tsp_did_not_create_is_never_written_into() {
        let dir = temp_dir("foreign");
        let input = hello(&dir);
        let out = dir.join("out");
        std::fs::create_dir_all(out.join("m")).unwrap();
        std::fs::write(out.join("m").join("keep.txt"), b"not yours").unwrap();

        // Even with --overwrite.
        assert_eq!(
            export(&input, &out, &opts(true, false)),
            ExportStatus::Refused
        );
        assert_eq!(tree_files(&out).len(), 1);
        assert_eq!(
            std::fs::read(out.join("m").join("keep.txt")).unwrap(),
            b"not yours"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_existing_target_directory_is_fine() {
        let dir = temp_dir("emptytarget");
        let input = hello(&dir);
        let out = dir.join("out");
        std::fs::create_dir_all(out.join("m")).unwrap();
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert!(out
            .join("m")
            .join("Hello World")
            .join("message.md")
            .exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_archive_of_a_different_source_is_refused() {
        let dir = temp_dir("mismatch");
        let input = hello(&dir);
        let out = dir.join("out");
        export(&input, &out, &opts(false, false));
        let before = tree_files(&out);

        // A directory called "m" has the same archive name as m.msg, but is a different source.
        let other = dir.join("other").join("m");
        std::fs::create_dir_all(&other).unwrap();
        write_msg(&other.join("x.msg"), "Different", Some("y"), false);
        assert_eq!(
            export(&other, &out, &opts(true, false)),
            ExportStatus::Refused
        );
        assert_eq!(tree_files(&out), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plan_that_does_not_fit_the_path_budget_writes_nothing() {
        let dir = temp_dir("budget");
        let input = hello(&dir);
        // Pad the absolute output path to 236 units. Staging still fits (limit 239), but the
        // archive root is then 238 units (`out\m`), so the shortest possible message directory
        // ("Hello World", 11 units) puts `metadata.json` at 238 + 1 + 11 + 1 + 13 = 264 > 259.
        let base = utf16_len(&std::path::absolute(&dir).unwrap().to_string_lossy());
        let pad = 236usize.saturating_sub(base + 1);
        assert!(pad > 0, "the temp directory path is too long for this test");
        let out = dir.join("o".repeat(pad));
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Refused
        );
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_output_path_too_long_to_stage_is_an_error() {
        let dir = temp_dir("toolong");
        let input = hello(&dir);
        let out = dir.join("o".repeat(260));
        assert!(run_export(&input, &out, &opts(false, false), &mut never_asked).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leftover_staged_file_from_an_interrupted_run_is_cleaned_up() {
        let dir = temp_dir("leftover");
        let input = hello(&dir);
        let out = dir.join("out");
        std::fs::create_dir_all(out.join(STAGING_DIR)).unwrap();
        std::fs::write(out.join(STAGING_DIR).join("7"), b"stale").unwrap();
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert!(!out.join(STAGING_DIR).exists());

        // Anything else in the staging directory is not ours: stop.
        std::fs::create_dir_all(out.join(STAGING_DIR)).unwrap();
        std::fs::write(out.join(STAGING_DIR).join("mine.txt"), b"x").unwrap();
        std::fs::write(
            out.join("m").join("Hello World").join("message.md"),
            b"changed",
        )
        .unwrap();
        assert!(run_export(&input, &out, &opts(true, false), &mut never_asked).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exporting_a_pst_is_not_implemented_yet() {
        let out = std::env::temp_dir().join("tsp-exp-never-created");
        let err = run_export(
            Path::new("x.pst"),
            &out,
            &opts(false, false),
            &mut never_asked,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not implemented"));
    }

    // ---- pure pieces --------------------------------------------------------

    fn pre(ownership: Ownership, replace: usize, blocked: usize) -> Preflight {
        Preflight {
            ownership,
            to_create: 1,
            identical: 0,
            to_replace: replace,
            blocked,
            unrelated_present: 0,
        }
    }

    #[test]
    fn the_consent_rules_cover_every_case() {
        use Decision::{Ask, Proceed, Refuse};
        use Ownership as Own;
        use RefuseReason as Why;
        assert_eq!(decide(&pre(Own::Absent, 0, 0), false, false), Proceed);
        assert_eq!(decide(&pre(Own::Empty, 0, 0), false, false), Proceed);
        assert_eq!(decide(&pre(Own::Owned, 0, 0), false, false), Proceed);
        assert_eq!(
            decide(&pre(Own::Owned, 3, 0), false, false),
            Refuse(Why::NeedsConsent)
        );
        assert_eq!(decide(&pre(Own::Owned, 3, 0), false, true), Ask);
        assert_eq!(decide(&pre(Own::Owned, 3, 0), true, false), Proceed);
        assert_eq!(decide(&pre(Own::Owned, 3, 0), true, true), Proceed);
        assert_eq!(
            decide(&pre(Own::NotOwned, 0, 0), true, true),
            Refuse(Why::NotOwned)
        );
        assert_eq!(
            decide(&pre(Own::OtherSource, 0, 0), true, true),
            Refuse(Why::SourceMismatch)
        );
        assert_eq!(
            decide(&pre(Own::Owned, 0, 2), true, true),
            Refuse(Why::Blocked)
        );
    }

    #[test]
    fn tree_hashes_depend_on_paths_and_content() {
        let a = vec![Generated {
            rel: vec!["x".to_string(), "message.md".to_string()],
            content: "one".to_string(),
        }];
        let mut b = a.clone();
        assert_eq!(tree_hash(&a), tree_hash(&b));
        b[0].content = "two".to_string();
        assert_ne!(tree_hash(&a), tree_hash(&b));
        let mut c = a.clone();
        c[0].rel[0] = "y".to_string();
        assert_ne!(tree_hash(&a), tree_hash(&c));
        assert_eq!(tree_hash(&a).len(), 64);
    }

    #[test]
    fn hex_encodes_lowercase_with_leading_zeros() {
        assert_eq!(hex([0x00u8, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn relative_source_names_are_never_absolute() {
        let dir = temp_dir("relname");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("sub").join("x.msg");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(relative_source_name(&dir, &file), "sub/x.msg");
        assert_eq!(relative_source_name(&file, &file), "x.msg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_command_line_accepts_the_export_flags_only_with_out() {
        assert!(Args::try_parse_from(["tsp", "x.msg", "--out", "o", "--overwrite"]).is_ok());
        assert!(Args::try_parse_from(["tsp", "x.msg", "--out", "o", "--no-source-hash"]).is_ok());
        assert!(Args::try_parse_from(["tsp", "x.msg", "--overwrite"]).is_err());
        assert!(Args::try_parse_from(["tsp", "x.msg", "--no-source-hash"]).is_err());
        assert!(
            Args::try_parse_from(["tsp", "x.msg", "--out", "o", "--dry-run", "--overwrite"])
                .is_err()
        );
        assert!(Args::try_parse_from(["tsp", "x.msg", "--out", "o", "--verify"]).is_err());
        let args = Args::try_parse_from([
            "tsp",
            "x.msg",
            "--out",
            "o",
            "--overwrite",
            "--no-source-hash",
        ])
        .unwrap();
        assert!(args.overwrite && args.no_source_hash && !args.dry_run);
    }
}
