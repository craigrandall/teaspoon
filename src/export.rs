//! Export writer (M4c walking skeleton, body added in M4e-2b, streamed in M4h-0, by-value
//! attachments added in M4f-1): `tsp <input.msg|dir> --out <dir>`.
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
//! Attachments (M4f-1): each top-level message's by-value attachments are copied into
//! `<message>/attachments/<planned name>` in chunks, one at a time, so memory does not grow with
//! attachment size; their size and SHA-256 are taken from what was written. Within a message the
//! attachments are staged first, then `metadata.json` (which records every attachment), then
//! `message.md` and the kept body files.
//!
//! Limits, stated plainly: only `.msg` input is exported, with its body (converted to Markdown
//! in `message.md`, plus each original body form kept as `body.html`, `body.rtf`, or `body.txt`)
//! and its by-value attachments. Embedded messages become nested message directories under
//! `<message>/attachments/` (M4f-2a, `export/nested.rs`; one the planner will not nest deeper gets
//! a `metadata.json` that says so). OLE objects and by-reference attachments
//! are counted and recorded as not extracted; a by-value attachment
//! whose data is missing, unreadable, or of another length than declared is recorded as failed
//! and the export goes on. Memory no longer grows with the archive (one message at a time, plus
//! the manifest), but `--out` and its `.tsp-tmp` staging directory are now created before the
//! consent decision and removed again if the run is refused; each file is replaced atomically,
//! but the archive as a whole is not all-or-nothing -- an interrupted run leaves the root
//! `folder.json` marked `incomplete`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::archive::{
    ATTACHMENT_EXTRACTED, ATTACHMENT_FAILED, ATTACHMENT_NOT_EXTRACTED, AttachmentRecord,
    BODY_HTML_NAME, BODY_RTF_NAME, BODY_TXT_NAME, BodyOutcome, ChildRecord, FOLDER_JSON_NAME,
    FolderMetadata, KIND_ARCHIVE_ROOT, NameRecord, RootCounts, RootRecord, RootSpec,
    STATUS_COMPLETE, STATUS_INCOMPLETE, SourceRecord, TOOL_NAME, adjustment_names, method_label,
    to_json,
};
use crate::body::{BodySource, LossReason};
use crate::dry_run::{SourceKind, classify_source, input_leaf_name, root_stem, root_units};
use crate::model::MessageContent;
use crate::naming::{collision_key, utf16_len};
use crate::plan::{
    EntryKind, MAX_PATH_UNITS, Plan, PlanEntry, Policy, SourceFolder, SourceMessage, plan_export,
    verify_plan,
};
use crate::shared::{
    ATTACH_METHOD_BY_REFERENCE, ATTACH_METHOD_BY_REFERENCE_ONLY,
    ATTACH_METHOD_BY_REFERENCE_RESOLVE, ATTACH_METHOD_BY_VALUE, ATTACH_METHOD_EMBEDDED_MESSAGE,
    ATTACH_METHOD_NONE, ATTACH_METHOD_OLE,
};
use crate::source_msg::{
    MsgAttachments, MsgFileMap, OpenMessage, build_msg_export_source, open_message,
};
use crate::stage::{
    Disposition, STAGING_TAIL_UNITS, SourceFailure, StagedFile, Stager, StreamOutcome, hash_file,
    hex, join_path,
};

mod nested;

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
    /// Source files or directory entries that could not be opened or read and are therefore not
    /// in the export (directory input; a single unopenable file is an error instead).
    source_open_errors: usize,
    /// Directories not entered because they lie below the directory depth cap.
    source_directories_beyond_depth_cap: usize,
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
    /// Planned attachment files that were not written (not extracted by design, failed, or
    /// inside an embedded message). Failed ones are a subset of this.
    attachments_not_extracted: usize,
    /// By-value attachments written to files.
    attachments_extracted: usize,
    /// Bytes of the by-value attachments written.
    attachment_bytes_extracted: u64,
    /// By-value attachments whose data was missing, unreadable, or of another length than
    /// declared.
    attachments_failed: usize,
    /// Embedded messages written as nested message directories (M4f-2a).
    embedded_messages_extracted: usize,
    /// Planned embedded messages not written as nested directories: past the planner's depth cap
    /// (a `metadata.json` only), unreadable, or inside one of those. Computed from the plan.
    embedded_messages_not_extracted: usize,
    /// Sum of the formatting-loss counts over the bodies of the embedded messages written (the
    /// per-reason body counters cover top-level messages only).
    embedded_messages_loss_total: u64,
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
    /// `cid:` links rewritten to the extracted attachment in top-level messages (M4f-2b); with
    /// the three unresolved counts below it sums to `body_cid_references_total`.
    cid_rewritten: u64,
    cid_unresolved_no_attachment: u64,
    cid_unresolved_not_extracted: u64,
    cid_unresolved_ambiguous: u64,
    /// Top-level attachments marked `inline`.
    attachments_inline: u64,
    /// The same three figures for the bodies of the embedded messages written (the unresolved
    /// links are summed over the three reasons).
    embedded_cid_rewritten: u64,
    embedded_cid_unresolved: u64,
    embedded_attachments_inline: u64,
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
        self.cid_rewritten += outcome.cid_links.rewritten;
        self.cid_unresolved_no_attachment += outcome.cid_links.unresolved_no_attachment;
        self.cid_unresolved_not_extracted += outcome.cid_links.unresolved_not_extracted;
        self.cid_unresolved_ambiguous += outcome.cid_links.unresolved_ambiguous;
        self.attachments_inline += outcome.cid_links.inline_attachments;
    }
}

fn print_export_report(r: &ExportReport) {
    println!("inventory=privacy_safe");
    println!("input_kind=msg_export");
    println!("export_root_units={}", r.root_units);
    println!("export_source_open_errors={}", r.source_open_errors);
    println!(
        "export_source_directories_beyond_depth_cap={}",
        r.source_directories_beyond_depth_cap
    );
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
    println!("export_attachments_extracted={}", r.attachments_extracted);
    println!(
        "export_attachment_bytes_extracted={}",
        r.attachment_bytes_extracted
    );
    println!("export_attachments_failed={}", r.attachments_failed);
    println!(
        "export_embedded_messages_extracted={}",
        r.embedded_messages_extracted
    );
    println!(
        "export_embedded_messages_not_extracted={}",
        r.embedded_messages_not_extracted
    );
    println!(
        "export_embedded_body_loss_total={}",
        r.embedded_messages_loss_total
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
    println!("export_cid_rewritten={}", r.cid_rewritten);
    println!(
        "export_cid_unresolved_no_attachment={}",
        r.cid_unresolved_no_attachment
    );
    println!(
        "export_cid_unresolved_not_extracted={}",
        r.cid_unresolved_not_extracted
    );
    println!(
        "export_cid_unresolved_ambiguous={}",
        r.cid_unresolved_ambiguous
    );
    println!("export_attachments_inline={}", r.attachments_inline);
    println!("export_embedded_cid_rewritten={}", r.embedded_cid_rewritten);
    println!(
        "export_embedded_cid_unresolved={}",
        r.embedded_cid_unresolved
    );
    println!(
        "export_embedded_attachments_inline={}",
        r.embedded_attachments_inline
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
// Attachments (M4f-1)
// -----------------------------------------------------------------------------

fn name_record(entry: &PlanEntry) -> NameRecord {
    NameRecord {
        original: entry.original.clone(),
        adjustments: adjustment_names(&entry.flags),
    }
}

/// Streams one by-value attachment into staging and fills in its record. A missing, unreadable,
/// or wrong-length data stream is a recorded failure; only a failure to write to staging is an
/// error.
fn stage_by_value(
    stager: &mut Stager,
    entry: &PlanEntry,
    index: usize,
    reader: &mut MsgAttachments,
    report: &mut ExportReport,
    record: &mut AttachmentRecord,
) -> Result<()> {
    let Some((mut data, declared)) = reader.open_data(index) else {
        record.status = ATTACHMENT_FAILED;
        record.reason = Some("data_stream_missing");
        report.attachments_failed += 1;
        return Ok(());
    };
    match stager.add_stream(entry.components.clone(), &mut data, Some(declared))? {
        StreamOutcome::Staged { size, digest } => {
            record.status = ATTACHMENT_EXTRACTED;
            record.file = entry.components.last().cloned();
            record.bytes = Some(size);
            record.sha256 = Some(hex(digest));
            report.attachments_extracted += 1;
            report.attachment_bytes_extracted += size;
        }
        StreamOutcome::SourceFailed(failure) => {
            record.status = ATTACHMENT_FAILED;
            record.reason = Some(match failure {
                SourceFailure::ReadError => "data_read_failed",
                SourceFailure::LengthMismatch => "data_length_mismatch",
            });
            report.attachments_failed += 1;
        }
    }
    Ok(())
}

/// Stages every by-value attachment of one message, one at a time, and returns a record for
/// every attachment (extracted or not), in storage order. Attachment bytes are never held whole.
fn stage_attachments(
    stager: &mut Stager,
    ctx: &nested::StageCtx<'_>,
    message: &SourceMessage,
    content: &MessageContent,
    reader: &mut MsgAttachments,
    report: &mut ExportReport,
) -> Result<Vec<AttachmentRecord>> {
    if content.attachments.len() != message.attachments.len() {
        bail!("a message changed while it was being exported; run the export again");
    }
    let mut records = Vec::with_capacity(message.attachments.len());
    for (index, (source, info)) in message
        .attachments
        .iter()
        .zip(&content.attachments)
        .enumerate()
    {
        let mut record = AttachmentRecord {
            name: NameRecord {
                original: String::new(),
                adjustments: Vec::new(),
            },
            method: method_label(info.method),
            method_code: info.method,
            content_id: info.content_id.clone(),
            inline: false,
            status: ATTACHMENT_NOT_EXTRACTED,
            reason: None,
            file: None,
            bytes: None,
            sha256: None,
        };
        if let Some(embedded) = &source.embedded {
            let entry = ctx
                .planned
                .get(&embedded.id)
                .ok_or_else(|| anyhow!("internal error: an embedded message is not planned"))?;
            record.name = name_record(entry);
            let at = nested::EmbeddedAt {
                message: embedded,
                entry,
                index,
            };
            nested::stage_embedded(stager, ctx, &at, reader, report, &mut record)?;
        } else {
            let entry = ctx
                .planned
                .get(&source.id)
                .ok_or_else(|| anyhow!("internal error: an attachment is not planned"))?;
            record.name = name_record(entry);
            match info.method {
                Some(ATTACH_METHOD_BY_VALUE) => {
                    stage_by_value(stager, entry, index, reader, report, &mut record)?;
                }
                Some(ATTACH_METHOD_NONE) => record.reason = Some("method_none"),
                Some(
                    ATTACH_METHOD_BY_REFERENCE
                    | ATTACH_METHOD_BY_REFERENCE_RESOLVE
                    | ATTACH_METHOD_BY_REFERENCE_ONLY,
                ) => record.reason = Some("by_reference"),
                Some(ATTACH_METHOD_OLE) => record.reason = Some("ole_object"),
                Some(ATTACH_METHOD_EMBEDDED_MESSAGE) => {
                    record.reason = Some("embedded_message_unreadable");
                }
                Some(_) => record.reason = Some("method_unrecognized"),
                None => record.reason = Some("method_unreadable"),
            }
        }
        records.push(record);
    }
    Ok(records)
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
/// one message's content is in memory at a time (an attachment is copied through in chunks).
/// Returns the root description, which the commit uses for the `incomplete` marker (the
/// complete root is staged like any other file, last).
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

    // The plan's attachment and embedded-message entries by source ID. Attachment IDs and
    // message IDs come from one counter, so they never collide.
    let mut planned: BTreeMap<u64, &PlanEntry> = BTreeMap::new();
    for entry in &plan.entries {
        if matches!(
            entry.kind,
            EntryKind::AttachmentFile | EntryKind::EmbeddedMessage
        ) {
            planned.insert(entry.source_id, entry);
        }
    }

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

    let mut root = RootSpec {
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
                attachments_extracted: 0,
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
                let OpenMessage {
                    content,
                    attachments: mut reader,
                } = open_message(path)?;
                // The attachments, `message.md`, `metadata.json`, and the kept body files, in
                // that order; embedded messages are staged by the same function, one level
                // below (`export/nested.rs`).
                let source_file = relative_source_name(input, path);
                let ctx = nested::StageCtx {
                    planned: &planned,
                    source_file: &source_file,
                };
                let job = nested::MessageJob {
                    components: &entry.components,
                    directory: name_record(entry),
                    embedded: false,
                };
                nested::stage_message(stager, &ctx, &job, message, &content, &mut reader, report)?;
            }
            // Counted from the plan after the loop; by-value attachments were staged with their
            // message above.
            EntryKind::AttachmentFile => {}
            EntryKind::EmbeddedMessage => {}
        }
    }

    report.attachments_not_extracted = plan
        .counters
        .attachment_files
        .saturating_sub(report.attachments_extracted);
    report.embedded_messages_not_extracted = plan
        .counters
        .embedded_messages
        .saturating_sub(report.embedded_messages_extracted);
    root.record.counts.attachments_extracted = report.attachments_extracted;
    root.record.counts.attachments_not_extracted = report.attachments_not_extracted;
    root.record.counts.embedded_messages_not_extracted = report.embedded_messages_not_extracted;

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
        source_open_errors: census.open_errors,
        source_directories_beyond_depth_cap: census.directories_beyond_depth_cap,
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
    if report.source_open_errors > 0 || report.source_directories_beyond_depth_cap > 0 {
        // Content-free: the counts are in the report above; no name or path is printed.
        eprintln!(
            "warning: some input was not exported and is not in the archive (see the export_source_* counts)"
        );
    }
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
    /// attachment (with no data stream).
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

    /// One attachment of a synthetic message.
    struct TestAttachment<'a> {
        method: i32,
        name: Option<&'a str>,
        /// `None` writes no data stream.
        data: Option<&'a [u8]>,
        content_id: Option<&'a str>,
    }

    fn by_value<'a>(name: &'a str, data: &'a [u8]) -> TestAttachment<'a> {
        TestAttachment {
            method: 1,
            name: Some(name),
            data: Some(data),
            content_id: None,
        }
    }

    fn add_test_attachment(
        comp: &mut cfb::CompoundFile<std::fs::File>,
        index: usize,
        a: &TestAttachment<'_>,
    ) {
        let storage = format!("/__attach_version1.0_#{index:08X}");
        comp.create_storage(&storage)
            .expect("create attachment storage");
        let mut props = vec![0u8; 8];
        let mut method = [0u8; 8];
        method[..4].copy_from_slice(&a.method.to_le_bytes());
        props.extend(entry(0x0003, 0x3705, method));
        put(comp, &format!("{storage}/__properties_version1.0"), &props);
        if let Some(name) = a.name {
            put(
                comp,
                &format!("{storage}/__substg1.0_3707001F"),
                &utf16(name),
            );
        }
        if let Some(data) = a.data {
            put(comp, &format!("{storage}/__substg1.0_37010102"), data);
        }
        if let Some(cid) = a.content_id {
            put(
                comp,
                &format!("{storage}/__substg1.0_3712001F"),
                &utf16(cid),
            );
        }
    }

    /// A plain message (subject and delivery time) with the given attachments.
    fn write_msg_attachments(path: &Path, subject: &str, attachments: &[TestAttachment<'_>]) {
        let mut comp = cfb::create(path).expect("create CFB");
        let ticks: u64 = 133_000_000_000_000_000;
        let mut props = vec![0u8; 32];
        props.extend(entry(0x0040, 0x0E06, ticks.to_le_bytes()));
        put(&mut comp, "/__properties_version1.0", &props);
        put(&mut comp, "/__substg1.0_0037001F", &utf16(subject));
        for (index, a) in attachments.iter().enumerate() {
            add_test_attachment(&mut comp, index, a);
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
        // The attachment is by value but its data stream is missing: nothing is written for it.
        assert!(
            !files.keys().any(|k| k.contains("attachments")),
            "no data, so no attachment file"
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
        assert_eq!(meta["attachments"][0]["status"], "failed");
        assert_eq!(meta["attachments"][0]["reason"], "data_stream_missing");
        assert_eq!(meta["attachments"][0]["name"]["original"], "report.pdf");
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

    // ---- attachments (M4f-1) -------------------------------------------------

    #[test]
    fn by_value_attachments_are_written_with_size_and_hash_and_recorded() {
        let dir = temp_dir("attachfiles");
        let file = dir.join("a.msg");
        let big: Vec<u8> = (0..(2 * 256 * 1024 + 123) as u32)
            .map(|i| (i % 251) as u8)
            .collect();
        write_msg_attachments(
            &file,
            "With files",
            &[
                by_value("report.pdf", &big),
                TestAttachment {
                    content_id: Some("logo@x"),
                    ..by_value("logo.png", b"\x89PNG")
                },
                by_value("empty.txt", b""),
            ],
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        assert_eq!(files["a/With files/attachments/report.pdf"], big);
        assert_eq!(files["a/With files/attachments/logo.png"], b"\x89PNG");
        assert_eq!(files["a/With files/attachments/empty.txt"], b"");
        assert!(!out.join(STAGING_DIR).exists());

        let meta = meta_of(&files, "a/With files/metadata.json");
        let records = meta["attachments"].as_array().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["status"], "extracted");
        assert_eq!(records[0]["file"], "report.pdf");
        assert_eq!(records[0]["bytes"], big.len());
        assert_eq!(records[0]["sha256"], hex(Sha256::digest(&big)));
        assert_eq!(records[0]["method"], "by_value");
        assert_eq!(records[1]["content_id"], "logo@x");
        assert_eq!(records[2]["bytes"], 0);
        assert_eq!(records[2]["sha256"], hex(Sha256::digest(b"")));
        assert_eq!(meta["attachments_not_extracted"], 0);
        assert_eq!(
            meta["status_reasons"],
            serde_json::json!(["other_properties_not_preserved"])
        );
        let root: serde_json::Value = serde_json::from_slice(&files["a/folder.json"]).unwrap();
        assert_eq!(root["root"]["counts"]["attachments_extracted"], 3);
        assert_eq!(root["root"]["counts"]["attachments_not_extracted"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_attachment_lands_under_its_own_planned_name() {
        let dir = temp_dir("attachdupes");
        let file = dir.join("d.msg");
        write_msg_attachments(
            &file,
            "Dupes",
            &[
                by_value("image001.png", b"first"),
                by_value("image001.png", b"second"),
                TestAttachment {
                    name: None,
                    ..by_value("", b"third")
                },
            ],
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        let files = tree_files(&out);
        let meta = meta_of(&files, "d/Dupes/metadata.json");
        let mut names = BTreeSet::new();
        for (i, expected) in [&b"first"[..], b"second", b"third"].iter().enumerate() {
            let name = meta["attachments"][i]["file"].as_str().expect("extracted");
            assert!(names.insert(name.to_string()), "names must be distinct");
            assert_eq!(files[&format!("d/Dupes/attachments/{name}")], *expected);
        }

        // A second run changes nothing and needs no consent.
        let before = tree_files(&out);
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attachments_that_cannot_be_extracted_are_recorded_with_a_reason() {
        let dir = temp_dir("attachmixed");
        let file = dir.join("x.msg");
        write_msg_attachments(
            &file,
            "Mixed",
            &[
                TestAttachment {
                    method: 6,
                    data: None,
                    ..by_value("obj.bin", b"")
                },
                TestAttachment {
                    method: 2,
                    data: None,
                    ..by_value("link.lnk", b"")
                },
                TestAttachment {
                    data: None,
                    ..by_value("gone.bin", b"")
                },
                by_value("ok.bin", b"fine"),
            ],
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        let files = tree_files(&out);
        let written: Vec<&str> = files
            .keys()
            .filter(|k| k.contains("/attachments/"))
            .map(String::as_str)
            .collect();
        assert_eq!(written, vec!["x/Mixed/attachments/ok.bin"]);

        let meta = meta_of(&files, "x/Mixed/metadata.json");
        let status: Vec<(&str, &str)> = meta["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["status"].as_str().unwrap(),
                    r["reason"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            status,
            vec![
                ("not_extracted", "ole_object"),
                ("not_extracted", "by_reference"),
                ("failed", "data_stream_missing"),
                ("extracted", ""),
            ]
        );
        assert_eq!(meta["attachments_not_extracted"], 3);
        assert!(
            meta["status_reasons"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("attachments_not_extracted"))
        );
        let root: serde_json::Value = serde_json::from_slice(&files["x/folder.json"]).unwrap();
        assert_eq!(root["root"]["counts"]["attachments_extracted"], 1);
        assert_eq!(root["root"]["counts"]["attachments_not_extracted"], 3);
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

    // ---- cid links (M4f-2b) --------------------------------------------------

    /// A message with a native HTML body and the given attachments.
    fn write_msg_html_attachments(
        path: &Path,
        subject: &str,
        html: &[u8],
        attachments: &[TestAttachment<'_>],
    ) {
        let mut comp = cfb::create(path).expect("create CFB");
        let ticks: u64 = 133_000_000_000_000_000;
        let mut props = vec![0u8; 32];
        props.extend(entry(0x0040, 0x0E06, ticks.to_le_bytes()));
        put(&mut comp, "/__properties_version1.0", &props);
        put(&mut comp, "/__substg1.0_0037001F", &utf16(subject));
        put(&mut comp, "/__substg1.0_10130102", html);
        for (index, a) in attachments.iter().enumerate() {
            add_test_attachment(&mut comp, index, a);
        }
    }

    #[test]
    fn cid_links_are_rewritten_to_the_extracted_attachment_and_it_is_marked_inline() {
        let dir = temp_dir("cidlinks");
        let file = dir.join("c.msg");
        let html: &[u8] = b"<p>Hi</p><p><img src=\"cid:logo@x\" alt=\"logo\"></p><p><img src=\"cid:gone@x\" alt=\"gone\"></p>";
        write_msg_html_attachments(
            &file,
            "Cid",
            html,
            &[
                TestAttachment {
                    content_id: Some("<logo@x>"),
                    ..by_value("my logo.png", b"\x89PNG")
                },
                by_value("plain.txt", b"text"),
                TestAttachment {
                    content_id: Some("gone@x"),
                    data: None,
                    ..by_value("gone.png", b"")
                },
            ],
        );
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        let meta = meta_of(&files, "c/Cid/metadata.json");
        let logo_file = meta["attachments"][0]["file"].as_str().expect("extracted");
        let link = crate::body::attachment_link(logo_file);
        let md = String::from_utf8(files["c/Cid/message.md"].clone()).unwrap();
        assert!(md.contains(&format!("({link})")), "{md}");
        assert!(!md.contains("(cid:logo@x)"));
        assert!(md.contains("(cid:gone@x)"));
        // The kept original is untouched.
        assert_eq!(files["c/Cid/body.html"].as_slice(), html);

        let cid = &meta["body"]["cid_references"];
        assert_eq!(cid["total"], 2);
        assert_eq!(cid["distinct"], 2);
        assert_eq!(cid["rewritten"], 1);
        assert_eq!(cid["unresolved_no_attachment"], 0);
        assert_eq!(cid["unresolved_not_extracted"], 1);
        assert_eq!(cid["unresolved_ambiguous"], 0);
        assert_eq!(meta["attachments"][0]["inline"], true);
        assert_eq!(meta["attachments"][1]["inline"], false);
        assert_eq!(meta["attachments"][2]["inline"], true);
        assert_eq!(meta["attachments"][2]["status"], "failed");

        // Deterministic, and a second run changes nothing.
        let before = tree_files(&out);
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- adversarial attachment names (M4f) -----------------------------------

    /// The Windows device names of rule N4 (the part before the first `.`, any case, including
    /// the superscript digits).
    fn is_reserved_device_name(name: &str) -> bool {
        let stem = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) {
            return true;
        }
        ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|rest| {
                let mut chars = rest.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some(c), None)
                        if c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}')
                )
            })
        })
    }

    /// What Windows (and rule N3) will not accept as a file name.
    fn assert_legal_windows_name(name: &str) {
        assert!(!name.is_empty(), "an empty name was written");
        assert!(name != "." && name != "..", "a dot name was written");
        for c in name.chars() {
            let code = c as u32;
            assert!(
                !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                    && code >= 0x20
                    && !(0x7f..=0x9f).contains(&code)
                    && !('\u{202a}'..='\u{202e}').contains(&c)
                    && !('\u{2066}'..='\u{2069}').contains(&c)
                    && !('\u{200b}'..='\u{200d}').contains(&c)
                    && c != '\u{feff}',
                "a forbidden character (U+{code:04X}) is in a written name"
            );
        }
        assert!(
            !name.ends_with('.') && !name.ends_with(' '),
            "a trailing dot or space is in a written name"
        );
        assert!(utf16_len(name) <= 255, "a component is over 255 units");
        assert!(
            !is_reserved_device_name(name),
            "a device name was written as a file name"
        );
    }

    #[test]
    fn adversarial_attachment_names_become_legal_unique_files_inside_attachments() {
        let dir = temp_dir("advnames");
        let file = dir.join("v.msg");
        let long = format!("{}.pdf", "x".repeat(300));
        let names: Vec<String> = vec![
            "CON.txt".into(),
            "nul".into(),
            "aux.tar.gz".into(),
            "LPT1".into(),
            "COM\u{b9}.txt".into(),
            long,
            "Report.pdf".into(),
            "report.PDF".into(),
            "caf\u{e9}.txt".into(),
            "cafe\u{301}.txt".into(),
            "bad\u{1}name.txt".into(),
            "tab\tname.txt".into(),
            "trailing dot.".into(),
            "trailing space ".into(),
            "a<b>c:d\"e|f?g*h.txt".into(),
            "desktop.ini".into(),
            "~$lock.docx".into(),
            "..".into(),
            ".".into(),
            "   ".into(),
            "\u{1F600}\u{1F600}.png".into(),
            "bidi\u{202e}gnp.exe".into(),
            "zero\u{200b}width.txt".into(),
            "../../escape.txt".into(),
            "..\\..\\escape2.txt".into(),
            "C:\\evil.txt".into(),
            "/abs/evil.txt".into(),
            "\\\\server\\share\\x.txt".into(),
        ];
        let datas: Vec<Vec<u8>> = (0..names.len())
            .map(|i| format!("data-{i}").into_bytes())
            .collect();
        let attachments: Vec<TestAttachment<'_>> = names
            .iter()
            .zip(&datas)
            .map(|(name, data)| by_value(name, data))
            .collect();
        write_msg_attachments(&file, "Adv", &attachments);
        let out = dir.join("out");
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );

        let files = tree_files(&out);
        let meta = meta_of(&files, "v/Adv/metadata.json");
        let records = meta["attachments"].as_array().unwrap();
        assert_eq!(records.len(), names.len());
        let prefix = "v/Adv/attachments/";
        let mut keys = BTreeSet::new();
        for (i, record) in records.iter().enumerate() {
            assert_eq!(record["status"], "extracted", "attachment {i}");
            let name = record["file"].as_str().expect("an extracted file name");
            assert_legal_windows_name(name);
            assert!(
                keys.insert(collision_key(name).to_string()),
                "attachment {i} shares a collision key with another"
            );
            assert_eq!(
                files[&format!("{prefix}{name}")],
                datas[i],
                "the bytes of attachment {i} are not under its planned name"
            );
            assert_eq!(record["bytes"], datas[i].len());
            assert_eq!(record["sha256"], hex(Sha256::digest(&datas[i])));
        }

        // One file per attachment, none merged or lost, none in a subdirectory.
        let written: Vec<&String> = files.keys().filter(|k| k.starts_with(prefix)).collect();
        assert_eq!(written.len(), names.len());
        for key in &written {
            assert!(
                !key[prefix.len()..].contains('/'),
                "a name created a subdirectory"
            );
        }
        // Nothing escaped the archive: a `..` or an absolute name must not reach outside it.
        for key in files.keys() {
            assert!(
                key.starts_with("v/"),
                "a file was written outside the archive root"
            );
        }
        for stray in ["escape.txt", "escape2.txt", "evil.txt", "x.txt"] {
            assert!(!dir.join(stray).exists(), "{stray} escaped to the parent");
            assert!(!out.join(stray).exists(), "{stray} escaped the archive");
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            2,
            "only the input and the output may exist"
        );

        // Every written path is within the Windows budget, measured independently of the plan.
        let absolute_out = std::path::absolute(&out).unwrap();
        for key in files.keys() {
            let units = utf16_len(&absolute_out.join(key).to_string_lossy());
            assert!(units <= 259, "a written path is {units} units");
        }

        // Deterministic: a repeat export writes nothing, and a second output is identical.
        assert_eq!(
            export(&file, &out, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out), files);
        let out_b = dir.join("out2");
        assert_eq!(
            export(&file, &out_b, &opts(false, false)),
            ExportStatus::Completed
        );
        assert_eq!(tree_files(&out_b), files);
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
    /// body. `TSP_BIG_ATTACH_MB` (default 0, none) adds one by-value attachment of that many
    /// megabytes to every message (M4f: use it with a small `TSP_BIG_COUNT`, since each message
    /// carries its own copy). Run by hand:
    /// `cargo test --release generate_a_large_synthetic_corpus -- --ignored`.
    #[test]
    #[ignore = "generates a large synthetic corpus by hand (memory gate)"]
    fn generate_a_large_synthetic_corpus() {
        let dir = PathBuf::from(
            std::env::var("TSP_BIG_CORPUS_DIR").expect("set TSP_BIG_CORPUS_DIR to a new directory"),
        );
        let env_usize = |key: &str, default: usize| -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let count = env_usize("TSP_BIG_COUNT", 200);
        let megabytes = env_usize("TSP_BIG_MB", 2);
        let attachment_mb = env_usize("TSP_BIG_ATTACH_MB", 0);
        std::fs::create_dir_all(&dir).expect("create the corpus directory");
        let attachment: Vec<u8> = (0..attachment_mb * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect();
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
            let path = dir.join(format!("big-{n:05}.msg"));
            write_msg_forms(
                &path,
                &format!("Big message {n:05}"),
                Some(&plain),
                Some(html.as_bytes()),
                None,
                Some(65001),
            );
            if attachment_mb > 0 {
                let mut comp = cfb::open_rw(&path).expect("reopen the message");
                add_test_attachment(&mut comp, 0, &by_value("big.bin", &attachment));
            }
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
