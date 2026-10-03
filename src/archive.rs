//! Pure rendering of the archive files (M4c): `message.md`, `metadata.json`, and `folder.json`.
//!
//! No I/O. Output is deterministic: struct fields serialize in declaration order, there are no
//! maps, no absolute paths, and no export-time values, so two exports of the same input are
//! byte-identical. The schemas are **drafts** (`schema_version` is `0.1-draft`); they are
//! frozen by the M4a-2 ADRs, after this walking skeleton has shown what they must hold.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{MessageContent, PlainBody};
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

/// `message.md` for M4c (provisional until the body-policy ADR, M4e): a heading with the subject,
/// then the plain-text body verbatim inside a `text` code fence so no Markdown syntax in the
/// body is interpreted. Line endings become LF and trailing newlines are trimmed. With no
/// plain text the file is the heading alone.
pub(crate) fn render_message_md(content: &MessageContent) -> String {
    let mut out = format!("# {}\n", title_for(content.subject.as_deref()));
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

/// `metadata.json` for M4c. The status is always `partial` because the envelope (sender,
/// recipients, headers), attachments, and formatted bodies are not extracted yet; each gap that
/// applies to this message is listed.
pub(crate) fn message_metadata(
    source_file: &str,
    content: &MessageContent,
    directory: NameRecord,
    attachments: usize,
) -> MessageMetadata {
    let (plain_text, markdown) = match &content.plain_body {
        PlainBody::Text(_) => ("present", "fenced_text_provisional"),
        PlainBody::Empty => ("empty", "title_only"),
        PlainBody::Absent => ("absent", "title_only"),
    };
    let mut status_reasons = vec!["envelope_not_extracted"];
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
        attachments_not_extracted: attachments,
        status: "partial",
        status_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BodyAvailability;

    fn content(subject: Option<&str>, body: PlainBody) -> MessageContent {
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
        }
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
            render_message_md(&content(Some("Only a title"), PlainBody::Absent)),
            "# Only a title\n"
        );
        assert_eq!(
            render_message_md(&content(None, PlainBody::Empty)),
            "# (no subject)\n"
        );
    }

    #[test]
    fn a_body_with_backticks_and_bare_carriage_returns_is_preserved_verbatim() {
        let md = render_message_md(&content(
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
                "envelope_not_extracted",
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
}
