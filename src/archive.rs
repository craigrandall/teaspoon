//! Pure rendering of the archive files (M4c): `message.md`, `metadata.json`, and `folder.json`,
//! and since M4e-2b the body: what is selected and converted, the original body files that are
//! kept, and the `body` object in `metadata.json`.
//!
//! No I/O. Output is deterministic: struct fields serialize in declaration order, there are no
//! maps, no absolute paths, and no export-time values, so two exports of the same input are
//! byte-identical. The schemas are **drafts** (`schema_version` is `0.1-draft`); they are
//! frozen by the M4a-2 ADRs, after this walking skeleton has shown what they must hold.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::body::{
    BodySource, ConversionFailed, Rendered, Selection, convert_html_isolated, count_cid_references,
    render_with, select_body,
};
use crate::model::{Address, BodyForms, Envelope, MessageContent, RecipientKind};
use crate::naming::NameFlag;

pub(crate) const SCHEMA_VERSION: &str = "0.1-draft";
pub(crate) const TOOL_NAME: &str = "teaspoon";
pub(crate) const FOLDER_JSON_NAME: &str = "folder.json";
pub(crate) const MESSAGE_MD_NAME: &str = "message.md";
pub(crate) const METADATA_JSON_NAME: &str = "metadata.json";
pub(crate) const KIND_ARCHIVE_ROOT: &str = "archive_root";
pub(crate) const STATUS_COMPLETE: &str = "complete";
pub(crate) const STATUS_INCOMPLETE: &str = "incomplete";
const UNTITLED: &str = "(no subject)";

/// The kept original body files (body ADR, decision 2). Each is shorter than `metadata.json`
/// (13 units), which already sets the worst-case child reservation.
pub(crate) const BODY_HTML_NAME: &str = "body.html";
pub(crate) const BODY_RTF_NAME: &str = "body.rtf";
pub(crate) const BODY_TXT_NAME: &str = "body.txt";

/// The converter recorded in the `body` object. Pinned `=0.5.4` in `Cargo.toml`; a change of the
/// pin must change this too (the converter ADR's revisit condition).
pub(crate) const CONVERTER_NAME: &str = "htmd";
pub(crate) const CONVERTER_VERSION: &str = "0.5.4";

const NOTE_AS_STORED: &str = "as_stored";
const NOTE_RECOVERED_FROM_RTF: &str = "recovered_from_rtf_charset_declaration_stale";

/// How a directory name was derived from the source name.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct NameRecord {
    /// The source name before any change (a subject or a folder name; may be empty).
    pub(crate) original: String,
    /// What the naming rules did to it, as stable snake_case labels.
    pub(crate) adjustments: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConverterRecord {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

/// One kept original body file.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OriginalRecord {
    pub(crate) file: &'static str,
    pub(crate) bytes: u64,
    /// SHA-256 of the file's bytes as lower-case hex.
    pub(crate) sha256: String,
    /// `as_stored` (native HTML bytes), `recovered_from_rtf_charset_declaration_stale` (HTML
    /// recovered from the RTF, UTF-8, whose `charset=` declaration is stale), or null.
    pub(crate) note: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CidRecord {
    pub(crate) total: u64,
    pub(crate) distinct: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LossRecord {
    pub(crate) reason: &'static str,
    pub(crate) count: u64,
}

/// The `body` object (draft schema; body ADR, decision 4). Field order is the serialization
/// order.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct BodyRecord {
    /// `native_html`, `rtf_html`, `plain_text`, or `none`: the source actually used.
    pub(crate) source: &'static str,
    /// Set when the Markdown in `message.md` came from a conversion.
    pub(crate) converter: Option<ConverterRecord>,
    /// Preprocessing steps that changed something, from the closed list.
    pub(crate) preprocessing: Vec<&'static str>,
    /// `PidTagInternetCodepage` as stored, when the message has one.
    pub(crate) internet_codepage: Option<u32>,
    /// Kept original files, in the order `body.html`, `body.rtf`, `body.txt`.
    pub(crate) originals: Vec<OriginalRecord>,
    /// `cid:` link destinations in `message.md` (they stay as written until M4f).
    pub(crate) cid_references: CidRecord,
    /// Formatting loss, in the vocabulary's order, only reasons with a count above zero.
    pub(crate) loss: Vec<LossRecord>,
    /// The kept files are byte copies of source content and are exempt from the file contract's
    /// UTF-8-without-BOM and LF rules.
    pub(crate) kept_files_exempt_from_file_rules: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AddressRecord {
    pub(crate) display_name: Option<String>,
    pub(crate) address_type: Option<String>,
    pub(crate) email_address: Option<String>,
    pub(crate) smtp_address: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RecipientRecord {
    /// `to`, `cc`, or `bcc`.
    pub(crate) kind: &'static str,
    #[serde(flatten)]
    pub(crate) address: AddressRecord,
}

/// The envelope as recorded in `metadata.json` (draft schema). Times are FILETIME ticks as the
/// source stores them, plus one derived UTC string for readers; importance and sensitivity are
/// the stored integers.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnvelopeRecord {
    pub(crate) sender: Option<AddressRecord>,
    pub(crate) sent_representing: Option<AddressRecord>,
    pub(crate) recipients: Vec<RecipientRecord>,
    pub(crate) recipients_unlisted: usize,
    pub(crate) submit_time_filetime: Option<i64>,
    pub(crate) delivery_time_filetime: Option<i64>,
    /// The submit time (else the delivery time) as `YYYY-MM-DDTHH:MM:SSZ`, or null when absent
    /// or not representable.
    pub(crate) date_utc: Option<String>,
    pub(crate) importance: Option<i32>,
    pub(crate) sensitivity: Option<i32>,
    pub(crate) conversation_topic: Option<String>,
    /// Lower-case hex of `PidTagConversationIndex`.
    pub(crate) conversation_index_hex: Option<String>,
    pub(crate) transport_headers: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessageMetadata {
    pub(crate) schema_version: &'static str,
    pub(crate) kind: &'static str,
    /// The `.msg` file this message came from: its name, or its path relative to the input
    /// directory (always `/`-separated). Never an absolute path.
    pub(crate) source_file: String,
    pub(crate) subject: Option<String>,
    pub(crate) internet_message_id: Option<String>,
    pub(crate) time_filetime: Option<i64>,
    pub(crate) directory: NameRecord,
    pub(crate) body: BodyRecord,
    pub(crate) envelope: EnvelopeRecord,
    pub(crate) attachments_not_extracted: usize,
    pub(crate) status: &'static str,
    pub(crate) status_reasons: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChildRecord {
    pub(crate) kind: &'static str,
    pub(crate) directory_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceRecord {
    /// `msg_file` or `msg_directory`.
    pub(crate) kind: &'static str,
    /// The input's own name (file name or directory name), never a path.
    pub(crate) name: String,
    pub(crate) size_bytes: u64,
    /// SHA-256 as lower-case hex; `null` when `--no-source-hash` was given.
    pub(crate) sha256: Option<String>,
    /// What the hash covers: `file`, or `listing_of_exported_msg_files` (the SHA-256 of
    /// lines `<relative path>\t<file SHA-256>\n` in sorted path order).
    pub(crate) sha256_scope: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RootCounts {
    pub(crate) folders: usize,
    pub(crate) messages: usize,
    pub(crate) attachments_not_extracted: usize,
    pub(crate) embedded_messages_not_extracted: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RootRecord {
    pub(crate) tool: &'static str,
    pub(crate) tool_version: &'static str,
    /// `incomplete` while an export is running (and if it was interrupted), `complete` after.
    pub(crate) status: &'static str,
    pub(crate) source: SourceRecord,
    pub(crate) max_path_units_allowed: usize,
    pub(crate) planned_longest_relative_path_units: usize,
    pub(crate) counts: RootCounts,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FolderMetadata {
    pub(crate) schema_version: &'static str,
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) directory: Option<NameRecord>,
    pub(crate) children: Vec<ChildRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) root: Option<RootRecord>,
}

impl FolderMetadata {
    pub(crate) fn folder(directory: NameRecord, children: Vec<ChildRecord>) -> Self {
        FolderMetadata {
            schema_version: SCHEMA_VERSION,
            kind: "folder",
            directory: Some(directory),
            children,
            root: None,
        }
    }

    pub(crate) fn archive_root(root: RootRecord, children: Vec<ChildRecord>) -> Self {
        FolderMetadata {
            schema_version: SCHEMA_VERSION,
            kind: KIND_ARCHIVE_ROOT,
            directory: None,
            children,
            root: Some(root),
        }
    }
}

/// Everything needed to render the root `folder.json` in either status.
#[derive(Debug, Clone)]
pub(crate) struct RootSpec {
    pub(crate) record: RootRecord,
    pub(crate) children: Vec<ChildRecord>,
}

impl RootSpec {
    pub(crate) fn render(&self, status: &'static str) -> Result<String> {
        let mut record = self.record.clone();
        record.status = status;
        to_json(&FolderMetadata::archive_root(record, self.children.clone()))
    }
}

/// Pretty JSON with two-space indentation, LF line endings, and one trailing newline.
pub(crate) fn to_json<T: Serialize>(value: &T) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value).context("failed to serialize JSON")?;
    text.push('\n');
    Ok(text)
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The naming flags as stable labels, in the flags' own (deterministic) order.
pub(crate) fn adjustment_names(flags: &BTreeSet<NameFlag>) -> Vec<String> {
    flags
        .iter()
        .map(|f| snake_case(&format!("{f:?}")))
        .collect()
}

/// The heading text: the subject with every whitespace run collapsed to one space and control
/// characters removed; `(no subject)` when nothing is left.
pub(crate) fn title_for(subject: Option<&str>) -> String {
    let words: Vec<&str> = subject
        .unwrap_or("")
        .split(char::is_whitespace)
        .filter(|w| !w.is_empty())
        .collect();
    let joined = words.join(" ");
    let cleaned: String = joined.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        UNTITLED.to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// A backtick fence longer than any backtick run in `text` (at least three).
fn fence_for(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// A Markdown code span that holds `text` exactly: the delimiter is one backtick longer than any
/// backtick run inside, and a value that starts or ends with a backtick is padded with a space.
fn code_span(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let ticks = "`".repeat(longest + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{ticks} {text} {ticks}")
    } else {
        format!("{ticks}{text}{ticks}")
    }
}

/// Whitespace runs collapsed to one space and control characters removed, so a value is one line.
fn one_line(text: &str) -> String {
    let joined = text
        .split(char::is_whitespace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().filter(|c| !c.is_control()).collect()
}

fn non_empty_line(text: Option<&str>) -> Option<String> {
    text.map(one_line).filter(|s| !s.is_empty())
}

/// How an address reads in `message.md`: `Name <email>`, the name alone, or the email alone.
/// The email shown is the SMTP address when there is one, else `PidTagEmailAddress` only when
/// the address type is `SMTP` (or absent); an `EX` address is an Exchange distinguished name,
/// not something a reader can use, so it is left to `metadata.json`.
fn address_text(address: &Address) -> String {
    let name = non_empty_line(address.display_name.as_deref());
    let email = non_empty_line(address.smtp_address.as_deref()).or_else(|| {
        let usable = match address.address_type.as_deref() {
            None => true,
            Some(kind) => kind.eq_ignore_ascii_case("SMTP"),
        };
        if usable {
            non_empty_line(address.email_address.as_deref())
        } else {
            None
        }
    });
    match (name, email) {
        (Some(n), Some(e)) if n == e => e,
        (Some(n), Some(e)) => format!("{n} <{e}>"),
        (Some(n), None) => n,
        (None, Some(e)) => e,
        (None, None) => "(unknown)".to_string(),
    }
}

/// The date shown for a message: the submit (sent) time, else the delivery time, as UTC.
fn envelope_date_utc(envelope: &Envelope) -> Option<String> {
    envelope
        .submit_time
        .or(envelope.delivery_time)
        .and_then(filetime_to_utc)
}

/// A FILETIME (100 ns ticks since 1601-01-01 UTC) as `YYYY-MM-DDTHH:MM:SSZ`; `None` for a
/// negative value or a year beyond 9999 (not representable in four digits).
pub(crate) fn filetime_to_utc(ticks: i64) -> Option<String> {
    if ticks < 0 {
        return None;
    }
    let seconds = ticks / 10_000_000;
    let days_since_1601 = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    // Days from 1601-01-01 to 1970-01-01.
    let days_since_1970 = days_since_1601 - 134_774;
    let (year, month, day) = civil_from_days(days_since_1970);
    if year > 9999 {
        return None;
    }
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The header lines shown under the heading: only what the message has, in a fixed order.
fn envelope_lines(envelope: &Envelope) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(sender) = &envelope.sender {
        lines.push(format!("- **From:** {}", code_span(&address_text(sender))));
    }
    if let Some(represented) = &envelope.sent_representing {
        lines.push(format!(
            "- **On behalf of:** {}",
            code_span(&address_text(represented))
        ));
    }
    for (kind, label) in [
        (RecipientKind::To, "To"),
        (RecipientKind::Cc, "Cc"),
        (RecipientKind::Bcc, "Bcc"),
    ] {
        let items: Vec<String> = envelope
            .recipients
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| code_span(&address_text(&r.address)))
            .collect();
        if !items.is_empty() {
            lines.push(format!("- **{label}:** {}", items.join(", ")));
        }
    }
    if let Some(date) = envelope_date_utc(envelope) {
        lines.push(format!("- **Date:** {date}"));
    }
    match envelope.importance {
        None | Some(1) => {}
        Some(0) => lines.push("- **Importance:** low".to_string()),
        Some(2) => lines.push("- **Importance:** high".to_string()),
        Some(n) => lines.push(format!("- **Importance:** unknown ({n})")),
    }
    match envelope.sensitivity {
        None | Some(0) => {}
        Some(1) => lines.push("- **Sensitivity:** personal".to_string()),
        Some(2) => lines.push("- **Sensitivity:** private".to_string()),
        Some(3) => lines.push("- **Sensitivity:** company confidential".to_string()),
        Some(n) => lines.push(format!("- **Sensitivity:** unknown ({n})")),
    }
    lines
}

// =============================================================================
// The body: selection, conversion, kept originals
// =============================================================================

/// One original body file to write next to `message.md`: a byte copy of source content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginalFile {
    pub(crate) name: &'static str,
    pub(crate) bytes: Vec<u8>,
    pub(crate) note: Option<&'static str>,
}

/// Everything the body contributes to one message, computed once so `message.md`,
/// `metadata.json`, the kept files, and the report all agree (and the converter runs once).
#[derive(Debug, Clone)]
pub(crate) struct BodyOutcome {
    pub(crate) rendered: Rendered,
    /// `body.html`, `body.rtf`, `body.txt`, in that order, each only if that form exists.
    pub(crate) originals: Vec<OriginalFile>,
    pub(crate) cid_total: u64,
    pub(crate) cid_distinct: u64,
}

/// Selects, converts, and collects the originals, with the production converter.
pub(crate) fn body_outcome(forms: &BodyForms) -> BodyOutcome {
    body_outcome_with(forms, &convert_html_isolated)
}

/// [`body_outcome`] with the converter supplied (so layout and failure paths can be tested).
pub(crate) fn body_outcome_with(
    forms: &BodyForms,
    convert: &dyn Fn(&str) -> Result<String, ConversionFailed>,
) -> BodyOutcome {
    let selection = select_body(forms);
    let rendered = render_with(&selection, convert);
    let originals = original_files(forms, &selection);
    let (cid_total, cid_distinct) = rendered
        .markdown
        .as_deref()
        .map_or((0, 0), count_cid_references);
    BodyOutcome {
        rendered,
        originals,
        cid_total,
        cid_distinct,
    }
}

/// The original body files (body ADR, decision 2): each form that exists (present and not
/// zero-length), whether or not it was selected. `body.html` is the native HTML as stored; when
/// there is none, the HTML recovered from the RTF (UTF-8). Never altered.
fn original_files(forms: &BodyForms, selection: &Selection) -> Vec<OriginalFile> {
    let mut files = Vec::new();
    let native = forms.native_html.as_ref().filter(|bytes| !bytes.is_empty());
    let recovered = if selection.source == BodySource::RtfHtml {
        selection.html.as_deref()
    } else {
        None
    };
    match (native, recovered) {
        (Some(bytes), _) => files.push(OriginalFile {
            name: BODY_HTML_NAME,
            bytes: bytes.clone(),
            note: Some(NOTE_AS_STORED),
        }),
        (None, Some(html)) => files.push(OriginalFile {
            name: BODY_HTML_NAME,
            bytes: html.as_bytes().to_vec(),
            note: Some(NOTE_RECOVERED_FROM_RTF),
        }),
        (None, None) => {}
    }
    if let Some(rtf) = forms.rtf.as_ref().filter(|bytes| !bytes.is_empty()) {
        files.push(OriginalFile {
            name: BODY_RTF_NAME,
            bytes: rtf.clone(),
            note: None,
        });
    }
    if let Some(text) = forms.plain_text.as_ref().filter(|text| !text.is_empty()) {
        files.push(OriginalFile {
            name: BODY_TXT_NAME,
            bytes: text.as_bytes().to_vec(),
            note: None,
        });
    }
    files
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `message.md`: a heading with the subject, a list of the envelope fields the message has (each
/// value in a code span so nothing in it is interpreted as Markdown), then the body. An HTML
/// source's Markdown follows as converted (LF line endings, no leading or trailing blank lines,
/// one final newline); a plain-text source is written verbatim inside a code fence longer than
/// any backtick run in it. A message with no envelope fields and no body is the heading alone.
pub(crate) fn render_message_md(content: &MessageContent, outcome: &BodyOutcome) -> String {
    let mut out = format!("# {}\n", title_for(content.subject.as_deref()));
    let lines = envelope_lines(&content.envelope);
    if !lines.is_empty() {
        out.push('\n');
        for line in &lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some(markdown) = &outcome.rendered.markdown {
        let normalized = normalize_newlines(markdown);
        let body = normalized.trim_start_matches('\n').trim_end();
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
    } else if let Some(text) = &outcome.rendered.plain_text {
        let normalized = normalize_newlines(text);
        let body = normalized.trim_end_matches('\n');
        let fence = fence_for(body);
        out.push('\n');
        out.push_str(&fence);
        out.push_str("text\n");
        out.push_str(body);
        out.push('\n');
        out.push_str(&fence);
        out.push('\n');
    }
    out
}

fn address_record(address: &Address) -> AddressRecord {
    AddressRecord {
        display_name: address.display_name.clone(),
        address_type: address.address_type.clone(),
        email_address: address.email_address.clone(),
        smtp_address: address.smtp_address.clone(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn envelope_record(envelope: &Envelope) -> EnvelopeRecord {
    EnvelopeRecord {
        sender: envelope.sender.as_ref().map(address_record),
        sent_representing: envelope.sent_representing.as_ref().map(address_record),
        recipients: envelope
            .recipients
            .iter()
            .map(|r| RecipientRecord {
                kind: match r.kind {
                    RecipientKind::To => "to",
                    RecipientKind::Cc => "cc",
                    RecipientKind::Bcc => "bcc",
                },
                address: address_record(&r.address),
            })
            .collect(),
        recipients_unlisted: envelope.recipients_unlisted,
        submit_time_filetime: envelope.submit_time,
        delivery_time_filetime: envelope.delivery_time,
        date_utc: envelope_date_utc(envelope),
        importance: envelope.importance,
        sensitivity: envelope.sensitivity,
        conversation_topic: envelope.conversation_topic.clone(),
        conversation_index_hex: envelope.conversation_index.as_deref().map(hex_lower),
        transport_headers: envelope.transport_headers.clone(),
    }
}

fn body_record(forms: &BodyForms, outcome: &BodyOutcome) -> BodyRecord {
    BodyRecord {
        source: outcome.rendered.source.label(),
        converter: outcome.rendered.markdown.as_ref().map(|_| ConverterRecord {
            name: CONVERTER_NAME,
            version: CONVERTER_VERSION,
        }),
        preprocessing: outcome.rendered.preprocessing.clone(),
        internet_codepage: forms.native_html_code_page,
        originals: outcome
            .originals
            .iter()
            .map(|original| OriginalRecord {
                file: original.name,
                bytes: original.bytes.len() as u64,
                sha256: sha256_hex(&original.bytes),
                note: original.note,
            })
            .collect(),
        cid_references: CidRecord {
            total: outcome.cid_total,
            distinct: outcome.cid_distinct,
        },
        loss: outcome
            .rendered
            .loss
            .iter()
            .map(|loss| LossRecord {
                reason: loss.reason.label(),
                count: loss.count,
            })
            .collect(),
        kept_files_exempt_from_file_rules: true,
    }
}

/// `metadata.json`. The status is always `partial` for now: properties the model does not yet
/// carry are not preserved (`other_properties_not_preserved`, always listed), and attachments
/// are not extracted yet; each gap that applies to this message is listed.
/// `formatted_bodies_not_converted` is listed only when an HTML or RTF form existed and the body
/// used ended up plain text or none (a failed conversion, an unusable RTF, or an RTF that could
/// not be decompressed); a converted body no longer carries it (body ADR, decision 4).
pub(crate) fn message_metadata(
    source_file: &str,
    content: &MessageContent,
    outcome: &BodyOutcome,
    directory: NameRecord,
    attachments: usize,
) -> MessageMetadata {
    let forms = &content.bodies;
    let formatted_form_existed =
        forms.native_html.is_some() || forms.rtf.is_some() || forms.rtf_decompression_failed;
    let fell_back = matches!(
        outcome.rendered.source,
        BodySource::PlainText | BodySource::None
    );
    let mut status_reasons = vec!["other_properties_not_preserved"];
    if attachments > 0 {
        status_reasons.push("attachments_not_extracted");
    }
    if formatted_form_existed && fell_back {
        status_reasons.push("formatted_bodies_not_converted");
    }
    MessageMetadata {
        schema_version: SCHEMA_VERSION,
        kind: "message",
        source_file: source_file.to_string(),
        subject: content.subject.clone(),
        internet_message_id: content.internet_message_id.clone(),
        time_filetime: content.time_filetime,
        directory,
        body: body_record(forms, outcome),
        envelope: envelope_record(&content.envelope),
        attachments_not_extracted: attachments,
        status: "partial",
        status_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Recipient;

    const ENCAPSULATED: &[u8] = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl{\f0\fswiss\fcharset0 Arial;}}{\*\htmltag64}{\*\htmltag84 <b>}Hello{\*\htmltag92 </b>}}";
    const PLAIN_RTF: &[u8] = br"{\rtf1\ansi Hello}";

    /// A message whose envelope holds only a delivery time (what the synthetic `.msg` files built
    /// by the export tests carry), so the pure renderer and a real export can share goldens.
    fn content(subject: Option<&str>, plain: Option<&str>) -> MessageContent {
        let mut c = bare(subject, plain);
        c.envelope.delivery_time = Some(133_000_000_000_000_000);
        c
    }

    /// A message with an empty envelope.
    fn bare(subject: Option<&str>, plain: Option<&str>) -> MessageContent {
        MessageContent {
            subject: subject.map(str::to_string),
            internet_message_id: None,
            time_filetime: Some(133_000_000_000_000_000),
            bodies: BodyForms {
                plain_text: plain.map(str::to_string),
                ..BodyForms::default()
            },
            envelope: Envelope::default(),
        }
    }

    fn md(c: &MessageContent) -> String {
        render_message_md(c, &body_outcome(&c.bodies))
    }

    fn meta_for(c: &MessageContent, original: &str, attachments: usize) -> MessageMetadata {
        message_metadata(
            "m.msg",
            c,
            &body_outcome(&c.bodies),
            NameRecord {
                original: original.to_string(),
                adjustments: vec![],
            },
            attachments,
        )
    }

    fn addr(
        name: Option<&str>,
        kind: Option<&str>,
        email: Option<&str>,
        smtp: Option<&str>,
    ) -> Address {
        Address {
            display_name: name.map(str::to_string),
            address_type: kind.map(str::to_string),
            email_address: email.map(str::to_string),
            smtp_address: smtp.map(str::to_string),
        }
    }

    /// A message that exercises every envelope field.
    fn full_envelope_content() -> MessageContent {
        let mut c = bare(Some("Budget review"), Some("Please review."));
        c.envelope = Envelope {
            sender: Some(addr(
                Some("Alice Sender"),
                Some("SMTP"),
                Some("alice@example.com"),
                Some("alice@example.com"),
            )),
            sent_representing: None,
            recipients: vec![
                Recipient {
                    kind: RecipientKind::To,
                    address: addr(
                        Some("Bob Receiver"),
                        Some("SMTP"),
                        None,
                        Some("bob@example.com"),
                    ),
                },
                Recipient {
                    kind: RecipientKind::To,
                    address: addr(None, Some("SMTP"), Some("carol@example.com"), None),
                },
                Recipient {
                    kind: RecipientKind::Cc,
                    address: addr(Some("Dan"), Some("EX"), Some("/O=EX/CN=DAN"), None),
                },
                Recipient {
                    kind: RecipientKind::Bcc,
                    address: addr(Some("Eve"), None, None, Some("eve@example.com")),
                },
            ],
            recipients_unlisted: 1,
            submit_time: Some(133_000_000_000_000_000),
            delivery_time: Some(133_000_000_100_000_000),
            importance: Some(2),
            sensitivity: Some(2),
            conversation_topic: Some("Budget review".to_string()),
            conversation_index: Some(vec![0x01, 0xAB, 0xCD]),
            transport_headers: Some("Received: from x\r\nSubject: Budget review".to_string()),
        };
        c
    }

    /// Golden files are compared with line endings normalized, in case a checkout converted them.
    fn lf(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    fn html_content(html: &[u8]) -> MessageContent {
        let mut c = content(Some("S"), None);
        c.bodies.native_html = Some(html.to_vec());
        c
    }

    #[test]
    fn titles_collapse_whitespace_and_fall_back() {
        assert_eq!(
            title_for(Some("  Re:\tHello \n World  ")),
            "Re: Hello World"
        );
        assert_eq!(title_for(Some("a\u{1}b")), "ab");
        assert_eq!(title_for(Some(" \t\n")), "(no subject)");
        assert_eq!(title_for(None), "(no subject)");
    }

    #[test]
    fn fences_are_longer_than_any_backtick_run_in_the_body() {
        assert_eq!(fence_for("plain"), "```");
        assert_eq!(fence_for("a ``` b"), "````");
        assert_eq!(fence_for("`` and ````` and `"), "``````");
    }

    #[test]
    fn the_message_markdown_matches_the_committed_golden_file() {
        let c = content(Some("Hello World"), Some("Line one\r\nLine two\r\n"));
        assert_eq!(md(&c), lf(include_str!("../tests/golden/hello/message.md")));
    }

    #[test]
    fn the_message_metadata_matches_the_committed_golden_file() {
        let c = content(Some("Hello World"), Some("Line one\r\nLine two\r\n"));
        let json = to_json(&meta_for(&c, "Hello World", 0)).unwrap();
        assert_eq!(
            json,
            lf(include_str!("../tests/golden/hello/metadata.json"))
        );
    }

    #[test]
    fn a_message_without_plain_text_is_the_heading_alone() {
        assert_eq!(md(&bare(Some("Only a title"), None)), "# Only a title\n");
        // A zero-length plain body is no body.
        assert_eq!(md(&bare(None, Some(""))), "# (no subject)\n");
    }

    #[test]
    fn a_body_with_backticks_and_bare_carriage_returns_is_preserved_verbatim() {
        let c = bare(Some("S"), Some("a ``` b\rsecond\n\n"));
        assert_eq!(md(&c), "# S\n\n````text\na ``` b\nsecond\n````\n");
    }

    #[test]
    fn converted_markdown_follows_the_envelope_normalized() {
        let c = html_content(b"<p>x</p>");
        let outcome = body_outcome_with(&c.bodies, &|_| {
            Ok("\n\r\nFirst\r\n\r\nSecond  \r\n\n\n".to_string())
        });
        assert_eq!(
            render_message_md(&c, &outcome),
            "# S\n\n- **Date:** 2022-06-18T04:26:40Z\n\nFirst\n\nSecond\n"
        );
    }

    #[test]
    fn blank_converted_markdown_leaves_the_heading_and_envelope_only() {
        let c = html_content(b"<p>x</p>");
        let outcome = body_outcome_with(&c.bodies, &|_| Ok("  \n".to_string()));
        assert_eq!(
            render_message_md(&c, &outcome),
            "# S\n\n- **Date:** 2022-06-18T04:26:40Z\n"
        );
    }

    #[test]
    fn a_failed_conversion_falls_back_to_the_fenced_plain_text() {
        let mut c = html_content(b"<p>x</p>");
        c.bodies.plain_text = Some("fallback".to_string());
        let outcome = body_outcome_with(&c.bodies, &|_| Err(ConversionFailed));
        assert_eq!(
            render_message_md(&c, &outcome),
            "# S\n\n- **Date:** 2022-06-18T04:26:40Z\n\n```text\nfallback\n```\n"
        );
        let meta = message_metadata(
            "m.msg",
            &c,
            &outcome,
            NameRecord {
                original: "S".to_string(),
                adjustments: vec![],
            },
            0,
        );
        assert_eq!(meta.body.source, "plain_text");
        assert!(meta.body.converter.is_none());
        assert_eq!(meta.body.loss.len(), 1);
        assert_eq!(meta.body.loss[0].reason, "html_conversion_failed");
        assert!(
            meta.status_reasons
                .contains(&"formatted_bodies_not_converted")
        );
    }

    #[test]
    fn a_converted_body_records_the_converter_and_drops_the_status_reason() {
        let c = html_content(b"<style>p{}</style><p>x</p>");
        let outcome = body_outcome_with(&c.bodies, &|_| Ok("x".to_string()));
        let meta = message_metadata(
            "m.msg",
            &c,
            &outcome,
            NameRecord {
                original: "S".to_string(),
                adjustments: vec![],
            },
            0,
        );
        assert_eq!(meta.body.source, "native_html");
        let converter = meta.body.converter.as_ref().unwrap();
        assert_eq!((converter.name, converter.version), ("htmd", "0.5.4"));
        assert_eq!(meta.body.preprocessing, vec!["style"]);
        assert_eq!(meta.status_reasons, vec!["other_properties_not_preserved"]);
    }

    #[test]
    fn originals_are_kept_verbatim_with_sizes_and_hashes() {
        let mut c = content(Some("S"), Some("caf\u{e9}"));
        c.bodies.native_html = Some(b"<p>caf\xe9</p>".to_vec());
        c.bodies.native_html_code_page = Some(1252);
        c.bodies.rtf = Some(ENCAPSULATED.to_vec());
        let outcome = body_outcome_with(&c.bodies, &|_| Ok("caf\u{e9}".to_string()));
        let names: Vec<&str> = outcome.originals.iter().map(|o| o.name).collect();
        assert_eq!(names, vec!["body.html", "body.rtf", "body.txt"]);
        assert_eq!(outcome.originals[0].bytes, b"<p>caf\xe9</p>");
        assert_eq!(outcome.originals[0].note, Some("as_stored"));
        assert_eq!(outcome.originals[1].bytes, ENCAPSULATED);
        assert_eq!(outcome.originals[2].bytes, "caf\u{e9}".as_bytes());

        let record = body_record(&c.bodies, &outcome);
        assert_eq!(record.internet_codepage, Some(1252));
        assert_eq!(record.originals[0].bytes, 11);
        assert_eq!(record.originals[0].sha256, sha256_hex(b"<p>caf\xe9</p>"));
        assert!(record.kept_files_exempt_from_file_rules);
    }

    #[test]
    fn the_hash_function_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn html_recovered_from_the_rtf_is_kept_when_there_is_no_native_html() {
        let mut c = content(Some("S"), None);
        c.bodies.rtf = Some(ENCAPSULATED.to_vec());
        let outcome = body_outcome_with(&c.bodies, &|_| Ok("**Hello**".to_string()));
        assert_eq!(outcome.rendered.source, BodySource::RtfHtml);
        assert_eq!(outcome.originals.len(), 2);
        assert_eq!(outcome.originals[0].name, "body.html");
        assert_eq!(outcome.originals[0].bytes, b"<b>Hello</b>");
        assert_eq!(
            outcome.originals[0].note,
            Some("recovered_from_rtf_charset_declaration_stale")
        );
        assert_eq!(outcome.originals[1].name, "body.rtf");
    }

    #[test]
    fn native_html_is_kept_even_when_the_rtf_html_was_selected() {
        let mut c = content(Some("S"), None);
        c.bodies.native_html = Some(b" \r\n ".to_vec());
        c.bodies.rtf = Some(ENCAPSULATED.to_vec());
        let outcome = body_outcome_with(&c.bodies, &|_| Ok("**Hello**".to_string()));
        assert_eq!(outcome.rendered.source, BodySource::RtfHtml);
        assert_eq!(outcome.originals[0].bytes, b" \r\n ");
        assert_eq!(outcome.originals[0].note, Some("as_stored"));
    }

    #[test]
    fn missing_and_zero_length_forms_keep_nothing() {
        let mut c = content(Some("S"), Some(""));
        c.bodies.native_html = Some(Vec::new());
        c.bodies.rtf = Some(Vec::new());
        let outcome = body_outcome_with(&c.bodies, &|_| Ok(String::new()));
        assert!(outcome.originals.is_empty());
        assert!(body_outcome(&BodyForms::default()).originals.is_empty());
    }

    #[test]
    fn a_whitespace_only_plain_body_is_kept_but_not_shown() {
        let c = content(Some("S"), Some(" \r\n"));
        let outcome = body_outcome(&c.bodies);
        assert_eq!(outcome.originals.len(), 1);
        assert_eq!(outcome.originals[0].bytes, b" \r\n");
        assert_eq!(
            render_message_md(&c, &outcome),
            "# S\n\n- **Date:** 2022-06-18T04:26:40Z\n"
        );
    }

    #[test]
    fn cid_references_are_counted_in_the_markdown() {
        let c = html_content(b"<p>x</p>");
        let outcome = body_outcome_with(&c.bodies, &|_| {
            Ok("![a](cid:one@x) ![b](cid:ONE@x) ![c](cid:two@x)".to_string())
        });
        assert_eq!((outcome.cid_total, outcome.cid_distinct), (3, 2));
        let record = body_record(&c.bodies, &outcome);
        assert_eq!(
            (record.cid_references.total, record.cid_references.distinct),
            (3, 2)
        );
    }

    #[test]
    fn gaps_are_listed_as_status_reasons() {
        let mut c = content(Some("S"), None);
        c.bodies.rtf = Some(PLAIN_RTF.to_vec());
        let meta = meta_for(&c, "S", 2);
        assert_eq!(meta.status, "partial");
        assert_eq!(
            meta.status_reasons,
            vec![
                "other_properties_not_preserved",
                "attachments_not_extracted",
                "formatted_bodies_not_converted"
            ]
        );
        assert_eq!(meta.body.source, "none");
        assert_eq!(meta.body.loss[0].reason, "rtf_not_converted");
        assert_eq!(meta.body.originals.len(), 1);
        assert_eq!(meta.body.originals[0].file, "body.rtf");
    }

    #[test]
    fn an_rtf_that_could_not_be_decompressed_is_recorded() {
        let mut c = content(Some("S"), Some("x"));
        c.bodies.rtf_decompression_failed = true;
        let meta = meta_for(&c, "S", 0);
        assert_eq!(meta.body.source, "plain_text");
        assert_eq!(meta.body.loss.len(), 1);
        assert_eq!(meta.body.loss[0].reason, "rtf_decompression_failed");
        assert!(
            meta.status_reasons
                .contains(&"formatted_bodies_not_converted")
        );
    }

    #[test]
    fn the_body_object_serializes_in_the_documented_order() {
        let c = content(Some("S"), Some("x"));
        let json = to_json(&meta_for(&c, "S", 0)).unwrap();
        let positions: Vec<usize> = [
            "\"source\": \"plain_text\"",
            "\"converter\"",
            "\"preprocessing\"",
            "\"internet_codepage\"",
            "\"originals\"",
            "\"cid_references\"",
            "\"loss\"",
            "\"kept_files_exempt_from_file_rules\"",
        ]
        .iter()
        .map(|key| json.find(key).unwrap_or_else(|| panic!("missing {key}")))
        .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn adjustment_labels_are_snake_case() {
        assert_eq!(snake_case("ReservedCharReplaced"), "reserved_char_replaced");
        assert_eq!(snake_case("Truncated"), "truncated");
    }

    #[test]
    fn the_root_renders_in_both_statuses_with_the_same_identity() {
        let spec = RootSpec {
            record: RootRecord {
                tool: TOOL_NAME,
                tool_version: "0.0.0",
                status: STATUS_COMPLETE,
                source: SourceRecord {
                    kind: "msg_file",
                    name: "m.msg".to_string(),
                    size_bytes: 10,
                    sha256: None,
                    sha256_scope: "file",
                },
                max_path_units_allowed: 259,
                planned_longest_relative_path_units: 30,
                counts: RootCounts {
                    folders: 0,
                    messages: 1,
                    attachments_not_extracted: 0,
                    embedded_messages_not_extracted: 0,
                },
            },
            children: vec![ChildRecord {
                kind: "message",
                directory_name: "Hello World".to_string(),
            }],
        };
        let done = spec.render(STATUS_COMPLETE).unwrap();
        let partial = spec.render(STATUS_INCOMPLETE).unwrap();
        assert!(done.contains("\"status\": \"complete\""));
        assert!(partial.contains("\"status\": \"incomplete\""));
        assert!(done.contains("\"kind\": \"archive_root\""));
        assert!(partial.contains("\"kind\": \"archive_root\""));
        assert!(done.contains("\"sha256\": null"));
        assert!(done.ends_with("}\n"));
        assert!(!done.contains('\r'));
    }

    #[test]
    fn file_times_convert_to_utc() {
        assert_eq!(filetime_to_utc(0).as_deref(), Some("1601-01-01T00:00:00Z"));
        assert_eq!(
            filetime_to_utc(116_444_736_000_000_000).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            filetime_to_utc(132_223_104_000_000_000).as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
        assert_eq!(
            filetime_to_utc(133_000_000_000_000_000).as_deref(),
            Some("2022-06-18T04:26:40Z")
        );
        // 2020-02-29 (a leap day) at 23:59:59.
        assert_eq!(
            filetime_to_utc(132_274_943_990_000_000).as_deref(),
            Some("2020-02-29T23:59:59Z")
        );
        assert_eq!(filetime_to_utc(-1), None);
        assert_eq!(filetime_to_utc(i64::MAX), None);
    }

    #[test]
    fn code_spans_hold_any_text_exactly() {
        assert_eq!(code_span("a b"), "`a b`");
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("`a"), "`` `a ``");
    }

    #[test]
    fn addresses_read_as_name_and_email_without_unusable_exchange_names() {
        let both = addr(Some("Alice"), Some("SMTP"), Some("a@x"), Some("a@x"));
        assert_eq!(address_text(&both), "Alice <a@x>");
        let name_is_email = addr(Some("a@x"), Some("SMTP"), Some("a@x"), None);
        assert_eq!(address_text(&name_is_email), "a@x");
        let exchange = addr(Some("Dan"), Some("EX"), Some("/O=EX/CN=DAN"), None);
        assert_eq!(address_text(&exchange), "Dan");
        let exchange_with_smtp = addr(Some("Dan"), Some("EX"), Some("/O=EX"), Some("d@x"));
        assert_eq!(address_text(&exchange_with_smtp), "Dan <d@x>");
        let email_only = addr(None, None, Some("e@x"), None);
        assert_eq!(address_text(&email_only), "e@x");
        assert_eq!(address_text(&Address::default()), "(unknown)");
        let multi_line = addr(Some("A\r\nB"), None, None, None);
        assert_eq!(address_text(&multi_line), "A B");
    }

    #[test]
    fn the_envelope_markdown_matches_the_committed_golden_file() {
        let md = md(&full_envelope_content());
        assert_eq!(md, lf(include_str!("../tests/golden/envelope/message.md")));
    }

    #[test]
    fn the_envelope_is_recorded_in_the_metadata() {
        let c = full_envelope_content();
        let meta = meta_for(&c, "Budget review", 0);
        let value: serde_json::Value = serde_json::from_str(&to_json(&meta).unwrap()).unwrap();
        let env = &value["envelope"];
        assert_eq!(env["sender"]["display_name"], "Alice Sender");
        assert_eq!(env["sender"]["smtp_address"], "alice@example.com");
        assert!(env["sent_representing"].is_null());
        assert_eq!(env["recipients"].as_array().unwrap().len(), 4);
        assert_eq!(env["recipients"][0]["kind"], "to");
        assert_eq!(env["recipients"][0]["display_name"], "Bob Receiver");
        assert_eq!(env["recipients"][2]["kind"], "cc");
        assert_eq!(env["recipients"][2]["address_type"], "EX");
        assert_eq!(env["recipients"][3]["kind"], "bcc");
        assert_eq!(env["recipients_unlisted"], 1);
        assert_eq!(env["submit_time_filetime"], 133_000_000_000_000_000i64);
        assert_eq!(env["delivery_time_filetime"], 133_000_000_100_000_000i64);
        assert_eq!(env["date_utc"], "2022-06-18T04:26:40Z");
        assert_eq!(env["importance"], 2);
        assert_eq!(env["sensitivity"], 2);
        assert_eq!(env["conversation_topic"], "Budget review");
        assert_eq!(env["conversation_index_hex"], "01abcd");
        assert_eq!(
            env["transport_headers"],
            "Received: from x\r\nSubject: Budget review"
        );
    }

    #[test]
    fn unusual_importance_and_sensitivity_values_are_shown_not_guessed() {
        let mut c = bare(Some("S"), None);
        c.envelope.importance = Some(7);
        c.envelope.sensitivity = Some(9);
        let shown = md(&c);
        assert!(shown.contains("- **Importance:** unknown (7)"));
        assert!(shown.contains("- **Sensitivity:** unknown (9)"));
        c.envelope.importance = Some(1);
        c.envelope.sensitivity = Some(0);
        assert_eq!(md(&c), "# S\n");
    }

    #[test]
    fn a_missing_or_unrepresentable_time_shows_no_date() {
        let mut c = bare(Some("S"), None);
        c.envelope.submit_time = Some(i64::MAX);
        assert_eq!(md(&c), "# S\n");
        assert!(envelope_record(&c.envelope).date_utc.is_none());
        assert_eq!(
            envelope_record(&c.envelope).submit_time_filetime,
            Some(i64::MAX)
        );
    }
}
