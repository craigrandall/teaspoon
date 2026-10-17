//! Export writer (M4c walking skeleton, body added in M4e-2b, streamed in M4h-0):
//! `tsp <input.msg|dir> --out <dir>`.
//!
//! Pipeline: read the source tree, plan names and paths (the same pure planner as `--dry-run`),
//! refuse if the plan breaks a gate, refuse early if the target directory is not ours, then
//! render each message once and write its files straight to the staging directory (`stage.rs`),
//! keeping only a manifest (path, size, SHA-256) in memory. The manifest is compared with what
//! is already on disk (preflight, counts only), consent is decided, the staged files are renamed
//! into place (identical ones are deleted from staging), and everything is read back by hash.
//! Archive content goes only into the directory the user named; stdout and stderr never carry
//! names, subjects, or paths.
//!
//! Limits, stated plainly: only `.msg` input is exported, with its body (converted to Markdown
//! in `message.md`, plus each original body form kept as `body.html`, `body.rtf`, or `body.txt`);
//! attachments and embedded messages are counted and recorded as not extracted (M4f). Memory no
//! longer grows with the archive (one message at a time, plus the manifest), but `--out` and its
//! `.tsp-tmp` staging directory are now created before the consent decision and removed again if
//! the run is refused; each file is replaced atomically, but the archive as a whole is not
//! all-or-nothing -- an interrupted run leaves the root `folder.json` marked `incomplete`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::archive::{
    BODY_HTML_NAME, BODY_RTF_NAME, BODY_TXT_NAME, BodyOutcome, ChildRecord, FOLDER_JSON_NAME,
    FolderMetadata, KIND_ARCHIVE_ROOT, MESSAGE_MD_NAME, METADATA_JSON_NAME, NameRecord, RootCounts,
    RootRecord, RootSpec, STATUS_COMPLETE, STATUS_INCOMPLETE, SourceRecord, TOOL_NAME,
    adjustment_names, body_outcome, message_metadata, render_message_md, to_json,
};
use crate::body::{BodySource, LossReason};
use crate::dry_run::{SourceKind, classify_source, input_leaf_name, root_stem, root_units};
use crate::naming::{collision_key, utf16_len};
use crate::plan::{EntryKind, Plan, Policy, SourceFolder, SourceMessage, plan_export, verify_plan};
use crate::source_msg::{MsgFileMap, build_msg_export_source, read_message_content};
use crate::stage::{
    Disposition, STAGING_TAIL_UNITS, StagedFile, Stager, hash_file, hex, join_path,
};

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

/// A preflight with no counts, for a refusal that happens before any file is compared.
fn empty_preflight(ownership: Ownership) -> Preflight {
    Preflight {
        ownership,
        to_create: 0,
        identical: 0,
        to_replace: 0,
        blocked: 0,
        unrelated_present: 0,
    }
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
    body_source_native_html: usize,
    body_source_rtf_html: usize,
    body_source_plain_text: usize,
    body_source_none: usize,
    body_files_kept_html: usize,
    body_files_kept_rtf: usize,
    body_files_kept_txt: usize,
    /// Sum of the loss counts per reason, in `LossReason::ALL` order.
    body_loss: [u64; LossReason::ALL.len()],
    body_cid_references_total: u64,
    readback_mismatches: usize,
    tree_sha256: String,
}

impl ExportReport {
    /// Tallies one message's body into the content-free counters.
    fn note_body(&mut self, outcome: &BodyOutcome) {
        match outcome.rendered.source {
            BodySource::NativeHtml => self.body_source_native_html += 1,
            BodySource::RtfHtml => self.body_source_rtf_html += 1,
            BodySource::PlainText => self.body_source_plain_text += 1,
            BodySource::None => self.body_source_none += 1,
        }
        for original in &outcome.originals {
            match original.name {
                BODY_HTML_NAME => self.body_files_kept_html += 1,
                BODY_RTF_NAME => self.body_files_kept_rtf += 1,
                BODY_TXT_NAME => self.body_files_kept_txt += 1,
                _ => {}
            }
        }
        for loss in &outcome.rendered.loss {
            if let Some(index) = LossReason::ALL.iter().position(|r| *r == loss.reason) {
                self.body_loss[index] += loss.count;
            }
        }
        self.body_cid_references_total += outcome.cid_total;
    }
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
    println!(
        "export_body_source_native_html={}",
        r.body_source_native_html
    );
    println!("export_body_source_rtf_html={}", r.body_source_rtf_html);
    println!("export_body_source_plain_text={}", r.body_source_plain_text);
    println!("export_body_source_none={}", r.body_source_none);
    println!("export_body_files_kept_html={}", r.body_files_kept_html);
    println!("export_body_files_kept_rtf={}", r.body_files_kept_rtf);
    println!("export_body_files_kept_txt={}", r.body_files_kept_txt);
    for (reason, total) in LossReason::ALL.iter().zip(r.body_loss) {
        println!("export_body_loss_{}={}", reason.label(), total);
    }
    println!(
        "export_body_cid_references_total={}",
        r.body_cid_references_total
    );
    println!("export_readback_mismatches={}", r.readback_mismatches);
    println!("export_tree_sha256={}", r.tree_sha256);
}

// -----------------------------------------------------------------------------
// Small helpers
// -----------------------------------------------------------------------------

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
    if input.is_dir()
        && let Ok(rel) = file.strip_prefix(input)
    {
        return rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
    }
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
// Rendering the archive into the staging directory, one message at a time
// -----------------------------------------------------------------------------

/// The read-only inputs of the staging pass, bundled so the function that stages stays short.
struct ArchiveInput<'a> {
    input: &'a Path,
    tree: &'a SourceFolder,
    plan: &'a Plan,
    policy: &'a Policy,
    sources: &'a MsgFileMap,
}

/// Renders every file of the archive and hands it to the stager the moment it exists, so only
/// one message's content is in memory at a time. Returns the root description, which the commit
/// uses for the `incomplete` marker (the complete root is staged like any other file, last).
fn stage_archive(
    archive: &ArchiveInput<'_>,
    source: SourceRecord,
    report: &mut ExportReport,
    stager: &mut Stager,
) -> Result<RootSpec> {
    let ArchiveInput {
        input,
        tree,
        plan,
        policy,
        sources,
    } = *archive;
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
                stager.add(
                    rel_with(&entry.components, FOLDER_JSON_NAME),
                    to_json(&meta)?.as_bytes(),
                )?;
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
                let outcome = body_outcome(&content.bodies);
                let meta = message_metadata(
                    &relative_source_name(input, path),
                    &content,
                    &outcome,
                    NameRecord {
                        original: entry.original.clone(),
                        adjustments: adjustment_names(&entry.flags),
                    },
                    message.attachments.len(),
                );
                stager.add(
                    rel_with(&entry.components, MESSAGE_MD_NAME),
                    render_message_md(&content, &outcome).as_bytes(),
                )?;
                stager.add(
                    rel_with(&entry.components, METADATA_JSON_NAME),
                    to_json(&meta)?.as_bytes(),
                )?;
                report.message_directories += 1;
                match content.bodies.plain_text.as_deref() {
                    None => report.bodies_plain_absent += 1,
                    Some(text) if text.trim().is_empty() => report.bodies_plain_empty += 1,
                    Some(_) => report.bodies_plain_present += 1,
                }
                report.note_body(&outcome);
                for original in &outcome.originals {
                    stager.add(rel_with(&entry.components, original.name), &original.bytes)?;
                }
            }
            EntryKind::AttachmentFile => report.attachments_not_extracted += 1,
            EntryKind::EmbeddedMessage => report.embedded_messages_not_extracted += 1,
        }
    }

    stager.add(
        vec![FOLDER_JSON_NAME.to_string()],
        root.render(STATUS_COMPLETE)?.as_bytes(),
    )?;
    report.folder_files += 1;
    stager.finish();
    Ok(root)
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

fn count_unrelated(root_dir: &Path, files: &[StagedFile]) -> usize {
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

/// Compares the manifest with the target by size and hash (an existing file is streamed through
/// the hasher, never held whole), and records for each staged file how it compares, so the
/// commit does not hash the target again.
fn compare_with_target(
    root_dir: &Path,
    ownership: Ownership,
    files: &[StagedFile],
) -> (Preflight, Vec<Disposition>) {
    let mut p = empty_preflight(ownership);
    let mut dispositions = Vec::with_capacity(files.len());
    for file in files {
        let path = join_path(root_dir, &file.rel);
        let disposition = match std::fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                p.to_create += 1;
                Disposition::Create
            }
            Err(_) => {
                p.blocked += 1;
                Disposition::Replace
            }
            Ok(meta) if !meta.is_file() => {
                p.blocked += 1;
                Disposition::Replace
            }
            Ok(meta) if meta.len() != file.size => {
                p.to_replace += 1;
                Disposition::Replace
            }
            Ok(_) => match hash_file(&path) {
                Ok((digest, _)) if digest == file.digest => {
                    p.identical += 1;
                    Disposition::Identical
                }
                Ok(_) => {
                    p.to_replace += 1;
                    Disposition::Replace
                }
                Err(_) => {
                    p.blocked += 1;
                    Disposition::Replace
                }
            },
        };
        dispositions.push(disposition);
    }
    if ownership == Ownership::Owned {
        p.unrelated_present = count_unrelated(root_dir, files);
    }
    (p, dispositions)
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

/// Drops the staged files after a refusal or a failure, and removes `--out` again if this run
/// created it and nothing else is in it.
fn abandon(stager: Stager, out: &Path, out_existed: bool) {
    stager.discard();
    if !out_existed {
        let _ = std::fs::remove_dir(out);
    }
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
        eprintln!(
            "{}",
            refusal_message(
                RefuseReason::PlanOverBudget,
                &empty_preflight(Ownership::Absent)
            )
        );
        return Ok(ExportStatus::Refused);
    }

    let source = source_record(input, &sources, options.source_hash)?;

    // A target that is not ours is refused before anything is rendered or staged.
    let root_dir = out.join(&stem);
    let ownership = classify_target(&root_dir, &source);
    report.target_state = ownership.label();
    let early_refusal = match ownership {
        Ownership::NotOwned => Some(RefuseReason::NotOwned),
        Ownership::OtherSource => Some(RefuseReason::SourceMismatch),
        Ownership::Absent | Ownership::Empty | Ownership::Owned => None,
    };
    if let Some(reason) = early_refusal {
        report.result = refusal_label(reason);
        print_export_report(&report);
        eprintln!("{}", refusal_message(reason, &empty_preflight(ownership)));
        return Ok(ExportStatus::Refused);
    }

    let out_existed = out.exists();
    let mut stager = Stager::begin(out)?;
    let archive = ArchiveInput {
        input,
        tree: &tree,
        plan: &plan,
        policy: &policy,
        sources: &sources,
    };
    let root = match stage_archive(&archive, source, &mut report, &mut stager) {
        Ok(root) => root,
        Err(error) => {
            abandon(stager, out, out_existed);
            return Err(error);
        }
    };
    report.tree_sha256 = stager.tree_hash();

    let (pre, dispositions) = compare_with_target(&root_dir, ownership, stager.files());
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
        abandon(stager, out, out_existed);
        print_export_report(&report);
        return Ok(ExportStatus::Refused);
    }

    let incomplete_root = root.render(STATUS_INCOMPLETE)?;
    let (written, unchanged) = stager.commit(
        &root_dir,
        incomplete_root.as_bytes(),
        &dispositions,
        pre.to_create + pre.to_replace,
    )?;
    report.files_written = written;
    report.files_unchanged = unchanged;
    report.readback_mismatches = stager.read_back_mismatches(&root_dir);
    print_export_report(&report);
    if report.readback_mismatches > 0 {
        bail!(
            "read-back found files that differ from what was planned; the archive is not trustworthy"
        );
    }
    Ok(ExportStatus::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use crate::stage::STAGING_DIR;
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

    /// A message with chosen body forms: plain text, native HTML bytes, the bytes of the
    /// compressed-RTF stream, and an Internet code page.
    fn write_msg_forms(
        path: &Path,
        subject: &str,
        plain: Option<&str>,
        html: Option<&[u8]>,
        rtf_stream: Option<&[u8]>,
        code_page: Option<i32>,
    ) {
        let mut comp = cfb::create(path).expect("create CFB");
        let ticks: u64 = 133_000_000_000_000_000;
        let mut props = vec![0u8; 32];
        props.extend(entry(0x0040, 0x0E06, ticks.to_le_bytes()));
        if let Some(cp) = code_page {
            let mut value = [0u8; 8];
            value[..4].copy_from_slice(&cp.to_le_bytes());
            props.extend(entry(0x0003, 0x3FDE, value));
        }
        put(&mut comp, "/__properties_version1.0", &props);
        put(&mut comp, "/__substg1.0_0037001F", &utf16(subject));
        if let Some(text) = plain {
            put(&mut comp, "/__substg1.0_1000001F", &utf16(text));
        }
        if let Some(bytes) = html {
            put(&mut comp, "/__substg1.0_10130102", bytes);
        }
        if let Some(bytes) = rtf_stream {
            put(&mut comp, "/__substg1.0_10090102", bytes);
        }
    }

    /// An MS-OXRTFCP "uncompressed" container (COMPTYPE `MELA`): the header, then the raw RTF.
    /// The tests that use it first check the premise that `compressed-rtf` accepts this form; if
    /// that check fails, replace this helper with an `LZFu` stream taken from a corpus file.
    fn mela(rtf: &[u8]) -> Vec<u8> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&(rtf.len() as u32 + 12).to_le_bytes());
        stream.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
        stream.extend_from_slice(b"MELA");
        stream.extend_from_slice(&0u32.to_le_bytes());
        stream.extend_from_slice(rtf);
        stream
    }

    fn assert_mela_premise(rtf: &[u8]) {
        assert!(
            compressed_rtf::decompress_rtf(&mela(rtf)).is_ok(),
            "premise: compressed-rtf accepts MELA (uncompressed) containers; if this fails, \
             replace mela() with an LZFu stream taken from a corpus file"
        );
    }

    const ENCAPSULATED: &[u8] = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl{\f0\fswiss\fcharset0 Arial;}}{\*\htmltag64}{\*\htmltag84 <b>}Hello{\*\htmltag92 </b>}}";

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

    fn meta_of(files: &BTreeMap<String, Vec<u8>>, key: &str) -> serde_json::Value {
        serde_json::from_slice(&files[key]).expect("metadata is JSON")
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
                "m/Hello World/body.txt",
                "m/Hello World/message.md",
                "m/Hello World/metadata.json",
                "m/folder.json"
            ]
        );
        // The kept original is a byte copy, CRLF and all (an inline constant, not a golden file:
        // a checkout that converts line endings would corrupt a golden `.txt`).
        assert_eq!(files["m/Hello World/body.txt"], b"Line one\r\nLine two\r\n");
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
        assert!(
            !out.join(STAGING_DIR).exists(),
            "the staged copies of identical files are removed"
        );
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
        assert!(
            !meta["directory"]["adjustments"]
                .as_array()
                .unwrap()
                .is_empty()
        );
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
            "# Empty one\n\n- **Date:** 2022-06-18T04:26:40Z\n"
        );
        let meta: serde_json::Value =
            serde_json::from_slice(&files["n/Empty one/metadata.json"]).unwrap();
        assert_eq!(meta["body"]["source"], "none");
        assert_eq!(meta["body"]["originals"], serde_json::json!([]));
        assert!(!files.keys().any(|k| k.contains("/body.")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- bodies (M4e-2b) ---------------------------------------------------

    #[test]
    fn a_native_html_message_exports_markdown_and_keeps_the_original_forms() {
        let dir = temp_dir("nativehtml");
        let file = dir.join("h.msg");
        let html: &[u8] = b"<html><head><title>T</title><style>p{}</style></head><body><p>Hello <b>world</b></p></body></html>";
        write_msg_forms(
            &file,
            "Page",
            Some("Hello world"),
            Some(html),
            None,
            Some(1252),
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec![
                "h/Page/body.html",
                "h/Page/body.txt",
                "h/Page/message.md",
                "h/Page/metadata.json",
                "h/folder.json"
            ]
        );
        assert_eq!(files["h/Page/body.html"].as_slice(), html);
        assert_eq!(files["h/Page/body.txt"].as_slice(), b"Hello world");
        let md = String::from_utf8(files["h/Page/message.md"].clone()).unwrap();
        assert!(md.starts_with("# Page\n\n- **Date:** 2022-06-18T04:26:40Z\n\n"));
        assert!(md.contains("Hello **world**"));
        assert!(!md.contains("p{}"));
        assert!(md.ends_with('\n') && !md.contains('\r'));

        let meta = meta_of(&files, "h/Page/metadata.json");
        let body = &meta["body"];
        assert_eq!(body["source"], "native_html");
        assert_eq!(body["converter"]["name"], "htmd");
        assert_eq!(body["converter"]["version"], "0.5.4");
        assert_eq!(body["preprocessing"], serde_json::json!(["style", "title"]));
        assert_eq!(body["internet_codepage"], 1252);
        assert_eq!(body["originals"][0]["file"], "body.html");
        assert_eq!(body["originals"][0]["bytes"], html.len());
        assert_eq!(body["originals"][0]["note"], "as_stored");
        assert_eq!(body["originals"][0]["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(body["originals"][1]["file"], "body.txt");
        assert_eq!(body["loss"], serde_json::json!([]));
        assert_eq!(
            meta["status_reasons"],
            serde_json::json!(["other_properties_not_preserved"])
        );

        // Deterministic, including the converter thread.
        let again = dir.join("again");
        export(&file, &again, &opts(false, false));
        assert_eq!(tree_files(&again), files);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_rtf_only_message_exports_the_recovered_html_and_the_rtf() {
        assert_mela_premise(ENCAPSULATED);
        let dir = temp_dir("rtfonly");
        let file = dir.join("r.msg");
        write_msg_forms(
            &file,
            "Rich",
            Some("Hello"),
            None,
            Some(&mela(ENCAPSULATED)),
            None,
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        assert_eq!(files["r/Rich/body.rtf"].as_slice(), ENCAPSULATED);
        assert_eq!(files["r/Rich/body.html"].as_slice(), b"<b>Hello</b>");
        let md = String::from_utf8(files["r/Rich/message.md"].clone()).unwrap();
        assert!(md.contains("**Hello**"));
        let body = meta_of(&files, "r/Rich/metadata.json")["body"].clone();
        assert_eq!(body["source"], "rtf_html");
        assert_eq!(
            body["originals"][0]["note"],
            "recovered_from_rtf_charset_declaration_stale"
        );
        assert_eq!(body["originals"][1]["file"], "body.rtf");
        assert_eq!(body["originals"][2]["file"], "body.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_rtf_without_html_is_kept_and_the_plain_text_is_used() {
        let rtf: &[u8] = br"{\rtf1\ansi Hello}";
        assert_mela_premise(rtf);
        let dir = temp_dir("plainrtf");
        let file = dir.join("p.msg");
        write_msg_forms(&file, "Plain", Some("Hello"), None, Some(&mela(rtf)), None);
        let out = dir.join("out");
        export(&file, &out, &opts(false, false));
        let files = tree_files(&out);
        assert_eq!(files["p/Plain/body.rtf"].as_slice(), rtf);
        assert!(!files.contains_key("p/Plain/body.html"));
        let meta = meta_of(&files, "p/Plain/metadata.json");
        assert_eq!(meta["body"]["source"], "plain_text");
        assert_eq!(meta["body"]["loss"][0]["reason"], "rtf_not_converted");
        assert!(
            meta["status_reasons"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("formatted_bodies_not_converted"))
        );
        let md = String::from_utf8(files["p/Plain/message.md"].clone()).unwrap();
        assert!(md.contains("```text\nHello\n```"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_rtf_that_cannot_be_decompressed_is_recorded_as_loss() {
        let dir = temp_dir("badrtf");
        let file = dir.join("b.msg");
        write_msg_forms(&file, "Broken", Some("Hello"), None, Some(b"short"), None);
        let out = dir.join("out");
        export(&file, &out, &opts(false, false));
        let files = tree_files(&out);
        assert!(!files.contains_key("b/Broken/body.rtf"));
        let meta = meta_of(&files, "b/Broken/metadata.json");
        assert_eq!(meta["body"]["source"], "plain_text");
        assert_eq!(
            meta["body"]["loss"][0]["reason"],
            "rtf_decompression_failed"
        );
        assert_eq!(meta["body"]["loss"][0]["count"], 1);
        assert!(
            meta["status_reasons"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("formatted_bodies_not_converted"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_body_counters_tally_source_files_loss_and_cid_references() {
        use crate::archive::body_outcome_with;
        use crate::model::BodyForms;
        let forms = BodyForms {
            native_html: Some(b"<p>x</p>".to_vec()),
            rtf: Some(br"{\rtf1\ansi Hello}".to_vec()),
            plain_text: Some("x".to_string()),
            ..BodyForms::default()
        };
        let outcome = body_outcome_with(&forms, &|_| Ok("![a](cid:one@x)".to_string()));
        let mut report = ExportReport::default();
        report.note_body(&outcome);
        report.note_body(&outcome);
        assert_eq!(report.body_source_native_html, 2);
        assert_eq!(report.body_source_plain_text, 0);
        assert_eq!(
            (
                report.body_files_kept_html,
                report.body_files_kept_rtf,
                report.body_files_kept_txt
            ),
            (2, 2, 2)
        );
        assert_eq!(report.body_cid_references_total, 2);
        assert_eq!(report.body_loss.iter().sum::<u64>(), 0);
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

        // Not interactive, no --overwrite: refused, file untouched, nothing left staged.
        std::fs::write(&target, b"edited by hand").unwrap();
        assert_eq!(
            export(&input, &out, &opts(false, false)),
            ExportStatus::Refused
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"edited by hand");
        assert!(!out.join(STAGING_DIR).exists());

        // Interactive and the user says no: refused, file untouched, nothing left staged.
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
        assert!(!out.join(STAGING_DIR).exists());

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

        // Even with --overwrite, and before anything is staged.
        assert_eq!(
            export(&input, &out, &opts(true, false)),
            ExportStatus::Refused
        );
        assert_eq!(tree_files(&out).len(), 1);
        assert!(!out.join(STAGING_DIR).exists());
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
        assert!(
            out.join("m")
                .join("Hello World")
                .join("message.md")
                .exists()
        );
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
        assert!(!out.join(STAGING_DIR).exists());
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

    // ---- the streamed pipeline (M4h-0) ---------------------------------------

    #[test]
    fn the_preflight_compares_the_manifest_with_the_target_by_size_and_hash() {
        let dir = temp_dir("compare");
        let out = dir.join("out");
        let root_dir = out.join("m");
        std::fs::create_dir_all(root_dir.join("a")).unwrap();
        std::fs::write(root_dir.join("a").join("same.txt"), b"same").unwrap();
        std::fs::write(root_dir.join("a").join("size.txt"), b"longer than staged").unwrap();
        std::fs::write(root_dir.join("a").join("hash.txt"), b"four").unwrap();
        std::fs::create_dir_all(root_dir.join("a").join("blocked.txt")).unwrap();
        std::fs::write(root_dir.join("stray.txt"), b"mine").unwrap();

        let mut stager = Stager::begin(&out).unwrap();
        for (name, content) in [
            ("same.txt", &b"same"[..]),
            ("size.txt", b"short"),
            ("hash.txt", b"FOUR"),
            ("blocked.txt", b"x"),
            ("new.txt", b"new"),
        ] {
            stager
                .add(vec!["a".to_string(), name.to_string()], content)
                .unwrap();
        }
        stager.finish();
        let (pre, dispositions) = compare_with_target(&root_dir, Ownership::Owned, stager.files());
        assert_eq!(
            (pre.identical, pre.to_replace, pre.to_create, pre.blocked),
            (1, 2, 1, 1)
        );
        assert_eq!(pre.unrelated_present, 1);
        assert_eq!(dispositions.len(), stager.files().len());
        assert_eq!(
            dispositions
                .iter()
                .filter(|d| **d == Disposition::Identical)
                .count(),
            1
        );
        stager.discard();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_streamed_export_hashes_the_tree_exactly_as_the_files_on_disk_do() {
        // Independent of the stager: the documented formula applied to the exported tree.
        let dir = temp_dir("treeformula");
        let input = hello(&dir);
        let out = dir.join("out");
        export(&input, &out, &opts(false, false));
        let mut expected = Sha256::new();
        for (path, bytes) in tree_files(&out.join("m")) {
            expected.update(path.as_bytes());
            expected.update([0u8]);
            expected.update(hex(Sha256::digest(&bytes)).as_bytes());
            expected.update(b"\n");
        }
        let on_disk = hex(expected.finalize());

        // The manifest's hash, from a second export staged and then discarded.
        let archive_dir = temp_dir("treeformula-b");
        let out_b = archive_dir.join("out");
        let stem = root_stem(&input);
        let policy = Policy::new(root_units(&out_b, &stem).unwrap());
        let (tree, _census, sources) = build_msg_export_source(&input).unwrap();
        let plan = plan_export(&tree, &policy);
        let source = source_record(&input, &sources, true).unwrap();
        let mut report = ExportReport::default();
        let mut stager = Stager::begin(&out_b).unwrap();
        let archive = ArchiveInput {
            input: &input,
            tree: &tree,
            plan: &plan,
            policy: &policy,
            sources: &sources,
        };
        stage_archive(&archive, source, &mut report, &mut stager).unwrap();
        assert_eq!(stager.tree_hash(), on_disk);
        stager.discard();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&archive_dir);
    }

    /// Generates a large synthetic corpus for the M4h-0 memory gate; it asserts nothing.
    /// `TSP_BIG_CORPUS_DIR` names a new directory; `TSP_BIG_COUNT` (default 200) is the number of
    /// messages and `TSP_BIG_MB` (default 2) the size of each message's HTML and plain-text
    /// body. Run by hand: `cargo test --release generate_a_large_synthetic_corpus -- --ignored`.
    #[test]
    #[ignore = "generates a large synthetic corpus by hand (M4h-0 memory gate)"]
    fn generate_a_large_synthetic_corpus() {
        let dir = PathBuf::from(
            std::env::var("TSP_BIG_CORPUS_DIR").expect("set TSP_BIG_CORPUS_DIR to a new directory"),
        );
        let count: usize = std::env::var("TSP_BIG_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let megabytes: usize = std::env::var("TSP_BIG_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        std::fs::create_dir_all(&dir).expect("create the corpus directory");
        for n in 0..count {
            let mut html = String::from("<html><body>");
            let mut plain = String::new();
            let mut i = 0usize;
            while html.len() < megabytes * 1024 * 1024 {
                html.push_str(&format!(
                    "<p>Message {n} paragraph {i}: the quick <b>brown</b> fox jumps over the <a href=\"http://example.com/{n}/{i}\">lazy</a> dog.</p>\n"
                ));
                plain.push_str(&format!(
                    "Message {n} paragraph {i}: the quick brown fox jumps over the lazy dog.\r\n"
                ));
                i += 1;
            }
            html.push_str("</body></html>");
            write_msg_forms(
                &dir.join(format!("big-{n:05}.msg")),
                &format!("Big message {n:05}"),
                Some(&plain),
                Some(html.as_bytes()),
                None,
                Some(65001),
            );
        }
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
