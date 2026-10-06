//! Pure rendering of the archive files (M4c): `message.md`, `metadata.json`, and `folder.json`.
//!
//! No I/O. Output is deterministic: struct fields serialize in declaration order, there are no
//! maps, no absolute paths, and no export-time values, so two exports of the same input are
//! byte-identical. The schemas are **drafts** (`schema_version` is `0.1-draft`); they are
//! frozen by the M4a-2 ADRs, after this walking skeleton has shown what they must hold.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{Address, Envelope, MessageContent, PlainBody, RecipientKind};
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

/// How a directory name was derived from the source name.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct NameRecord {
    /// The source name before any change (a subject or a folder name; may be empty).
    pub(crate) original: String,
    /// What the naming rules did to it, as stable snake_case labels.
    pub(crate) adjustments: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BodyRecord {
    /// `present`, `empty`, or `absent`.
    pub(crate) plain_text: &'static str,
    /// How the plain text appears in `message.md`.
    pub(crate) markdown: &'static str,
    pub(crate) html_native: bool,
    pub(crate) html_via_rtf: bool,
    pub(crate) rtf: bool,
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

/// `message.md` (provisional until the body-policy ADR, M4e): a heading with the subject, a list
/// of the envelope fields the message has (each value in a code span so nothing in it is
/// interpreted as Markdown), then the plain-text body verbatim inside a `text` code fence. Line
/// endings become LF and trailing newlines are trimmed. A message with no envelope fields and no
/// plain text is the heading alone.
pub(crate) fn render_message_md(content: &MessageContent) -> String {
    let mut out = format!("# {}\n", title_for(content.subject.as_deref()));
    let lines = envelope_lines(&content.envelope);
    if !lines.is_empty() {
        out.push('\n');
        for line in &lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let PlainBody::Text(text) = &content.plain_body {
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

/// `metadata.json`. The status is always `partial` for now: properties the model does not yet
/// carry are not preserved (`other_properties_not_preserved`, always listed), and attachments and
/// formatted bodies are not extracted yet; each gap that applies to this message is listed.
pub(crate) fn message_metadata(
    source_file: &str,
    content: &MessageContent,
    directory: NameRecord,
    attachments: usize,
) -> MessageMetadata {
    let (plain_text, markdown) = match &content.plain_body {
        PlainBody::Text(_) => ("present", "fenced_text_provisional"),
        PlainBody::Empty => ("empty", "no_body_text"),
        PlainBody::Absent => ("absent", "no_body_text"),
    };
    let mut status_reasons = vec!["other_properties_not_preserved"];
    if attachments > 0 {
        status_reasons.push("attachments_not_extracted");
    }
    if content.bodies.html_native || content.bodies.html_via_rtf {
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
        body: BodyRecord {
            plain_text,
            markdown,
            html_native: content.bodies.html_native,
            html_via_rtf: content.bodies.html_via_rtf,
            rtf: content.bodies.rtf,
        },
        envelope: envelope_record(&content.envelope),
        attachments_not_extracted: attachments,
        status: "partial",
        status_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BodyAvailability, Recipient};

    /// A message whose envelope holds only a delivery time (what the synthetic `.msg` files built
    /// by the export tests carry), so the pure renderer and a real export can share goldens.
    fn content(subject: Option<&str>, body: PlainBody) -> MessageContent {
        let mut c = bare(subject, body);
        c.envelope.delivery_time = Some(133_000_000_000_000_000);
        c
    }

    /// A message with an empty envelope.
    fn bare(subject: Option<&str>, body: PlainBody) -> MessageContent {
        MessageContent {
            subject: subject.map(str::to_string),
            internet_message_id: None,
            time_filetime: Some(133_000_000_000_000_000),
            plain_body: body,
            bodies: BodyAvailability {
                html_native: false,
                html_via_rtf: false,
                rtf: false,
            },
            envelope: Envelope::default(),
        }
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
        let mut c = bare(
            Some("Budget review"),
            PlainBody::Text("Please review.".to_string()),
        );
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
        let md = render_message_md(&content(
            Some("Hello World"),
            PlainBody::Text("Line one\r\nLine two\r\n".to_string()),
        ));
        assert_eq!(md, lf(include_str!("../tests/golden/hello/message.md")));
    }

    #[test]
    fn the_message_metadata_matches_the_committed_golden_file() {
        let c = content(
            Some("Hello World"),
            PlainBody::Text("Line one\r\nLine two\r\n".to_string()),
        );
        let meta = message_metadata(
            "m.msg",
            &c,
            NameRecord {
                original: "Hello World".to_string(),
                adjustments: vec![],
            },
            0,
        );
        let json = to_json(&meta).unwrap();
        assert_eq!(
            json,
            lf(include_str!("../tests/golden/hello/metadata.json"))
        );
    }

    #[test]
    fn a_message_without_plain_text_is_the_heading_alone() {
        assert_eq!(
            render_message_md(&bare(Some("Only a title"), PlainBody::Absent)),
            "# Only a title\n"
        );
        assert_eq!(
            render_message_md(&bare(None, PlainBody::Empty)),
            "# (no subject)\n"
        );
    }

    #[test]
    fn a_body_with_backticks_and_bare_carriage_returns_is_preserved_verbatim() {
        let md = render_message_md(&bare(
            Some("S"),
            PlainBody::Text("a ``` b\rsecond\n\n".to_string()),
        ));
        assert_eq!(md, "# S\n\n````text\na ``` b\nsecond\n````\n");
    }

    #[test]
    fn gaps_are_listed_as_status_reasons() {
        let mut c = content(Some("S"), PlainBody::Absent);
        c.bodies.html_via_rtf = true;
        let meta = message_metadata(
            "m.msg",
            &c,
            NameRecord {
                original: "S".to_string(),
                adjustments: vec![],
            },
            2,
        );
        assert_eq!(meta.status, "partial");
        assert_eq!(
            meta.status_reasons,
            vec![
                "other_properties_not_preserved",
                "attachments_not_extracted",
                "formatted_bodies_not_converted"
            ]
        );
        assert_eq!(meta.body.plain_text, "absent");
        assert!(meta.body.html_via_rtf);
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
        let md = render_message_md(&full_envelope_content());
        assert_eq!(md, lf(include_str!("../tests/golden/envelope/message.md")));
    }

    #[test]
    fn the_envelope_is_recorded_in_the_metadata() {
        let c = full_envelope_content();
        let meta = message_metadata(
            "e.msg",
            &c,
            NameRecord {
                original: "Budget review".to_string(),
                adjustments: vec![],
            },
            0,
        );
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
        let mut c = bare(Some("S"), PlainBody::Absent);
        c.envelope.importance = Some(7);
        c.envelope.sensitivity = Some(9);
        let md = render_message_md(&c);
        assert!(md.contains("- **Importance:** unknown (7)"));
        assert!(md.contains("- **Sensitivity:** unknown (9)"));
        c.envelope.importance = Some(1);
        c.envelope.sensitivity = Some(0);
        assert_eq!(render_message_md(&c), "# S\n");
    }

    #[test]
    fn a_missing_or_unrepresentable_time_shows_no_date() {
        let mut c = bare(Some("S"), PlainBody::Absent);
        c.envelope.submit_time = Some(i64::MAX);
        assert_eq!(render_message_md(&c), "# S\n");
        assert!(envelope_record(&c.envelope).date_utc.is_none());
        assert_eq!(
            envelope_record(&c.envelope).submit_time_filetime,
            Some(i64::MAX)
        );
    }
}
