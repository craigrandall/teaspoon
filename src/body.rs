//! The body pipeline (M4e-2a, wired into the export in M4e-2b): the pure part. Decided by the
//! accepted ADRs [HTML-to-Markdown converter and preprocessing] and [Body selection, original
//! bodies, and formatting-loss recording].
//!
//! No file I/O and no adapter types: the `.msg` reader (and later the PST reader) fills a
//! [`BodyForms`] (defined in `model.rs`), [`select_body`] picks the body, and [`render_body`]
//! turns it into what `message.md` and `metadata.json` need, with the loss recorded.
//!
//! - **Selection** (ADR 2, decision 1): native HTML, else HTML recovered from the RTF, else the
//!   plain-text body, else none. An RTF that could not be used is flagged, not converted.
//! - **Preprocessing** (converter ADR, decision 2, amended by body ADR decision 8): comments and
//!   `style`, `script`, and `title` elements are removed with their text in one left-to-right
//!   pass, then Word's downlevel markers and `o:p` tags. The bake-off harness shares this
//!   function, so its figures measure what production does.
//! - **Conversion**: `htmd` (pinned `=0.5.4`, default mode) on a worker thread with a large
//!   stack, because it recurses on nesting depth and a panic must not take the process down; a
//!   failure is recorded and falls back to the plain-text body.
//! - **Invisible padding** (ADR 2, decision 5): runs of invisible formatting characters are
//!   removed from the Markdown only; the original body is untouched (the caller keeps it).
//! - **Loss** (ADR 2, decision 4, amended twice): a closed list of reasons with counts.
//!
//! Not here: reading the forms out of a message (`oxmsg_extract::read_body_forms`), the kept
//! original files and the `metadata.json` object (`archive.rs`), and the content-preservation
//! check (M4e-2c).

use std::collections::BTreeSet;
use std::sync::Mutex;

use crate::model::BodyForms;
use crate::oxmsg_decode::{String8Decoded, decode_string8_with_codepage};
use crate::rtf_deencap::{Limits, RtfKind, deencapsulate_html, recognize};

/// Stack for the conversion thread (the bake-off harness uses the same size; Windows' default
/// main-thread stack is only 1 MiB and `htmd` recurses on nesting depth).
const STACK_BYTES: usize = 32 * 1024 * 1024;

/// The fewest invisible characters that make a run "padding" (ADR 2, decision 5). The runs seen
/// are 200 long; emoji joiner sequences are never 8 invisible characters in a row.
const PADDING_MIN_RUN: usize = 8;

/// Serializes the temporary silencing of the panic hook around a conversion, so a panic inside
/// the converter cannot print content to stderr and concurrent conversions (in tests) cannot
/// restore each other's hooks out of order.
static HOOK_LOCK: Mutex<()> = Mutex::new(());

// =============================================================================
// Selection
// =============================================================================

/// Which form became the body in `message.md`. The label is what `metadata.json` records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BodySource {
    NativeHtml,
    RtfHtml,
    PlainText,
    None,
}

impl BodySource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            BodySource::NativeHtml => "native_html",
            BodySource::RtfHtml => "rtf_html",
            BodySource::PlainText => "plain_text",
            BodySource::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) source: BodySource,
    /// The HTML to convert, as UTF-8 text (native HTML decoded, or the HTML recovered from RTF);
    /// set for the two HTML sources only.
    pub(crate) html: Option<String>,
    /// The plain-text body when it is non-empty, whichever source was selected (it is the
    /// fallback when a conversion fails).
    pub(crate) plain_text: Option<String>,
    /// An RTF existed and was not used (and no HTML was selected).
    pub(crate) rtf_not_converted: bool,
    /// The kind of the RTF, when the message has one. Read by tests; the export records the RTF
    /// kind when M4g settles the status vocabulary.
    #[allow(dead_code)]
    pub(crate) rtf_kind: Option<RtfKind>,
    /// A non-empty RTF existed and could not be decompressed (recorded as loss).
    pub(crate) rtf_decompression_failed: bool,
    /// Bytes replaced by U+FFFD while reading the selected HTML (a code page that is not
    /// implemented, invalid UTF-8, or the de-encapsulation's own count).
    pub(crate) undecodable_bytes: u64,
}

fn nonblank(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Native HTML bytes as UTF-8 text and the number of bytes replaced by U+FFFD. The code page is
/// `PidTagInternetCodepage`; absent, zero, or 65001 means UTF-8. A code page that
/// `oxmsg_decode` does not implement gets its non-ASCII bytes replaced and counted, never
/// guessed. A leading byte order mark is dropped. Not yet confirmed on real data: the corpus has
/// no native HTML.
pub(crate) fn decode_native_html(bytes: &[u8], code_page: Option<u32>) -> (String, u64) {
    let (text, replaced) = match code_page {
        None | Some(0) | Some(65001) => match std::str::from_utf8(bytes) {
            Ok(text) => (text.to_string(), 0),
            Err(_) => {
                let lossy = String::from_utf8_lossy(bytes).into_owned();
                let replaced = lossy.matches('\u{FFFD}').count() as u64;
                (lossy, replaced)
            }
        },
        Some(code_page) => match decode_string8_with_codepage(bytes, code_page) {
            String8Decoded::Decoded { text, .. }
            | String8Decoded::AsciiUnderUnsupportedCodepage { text } => (text, 0),
            String8Decoded::UnsupportedCodepage => {
                let mut text = String::with_capacity(bytes.len());
                let mut replaced = 0u64;
                for &byte in bytes {
                    if byte < 0x80 {
                        text.push(char::from(byte));
                    } else {
                        text.push('\u{FFFD}');
                        replaced += 1;
                    }
                }
                (text, replaced)
            }
        },
    };
    match text.strip_prefix('\u{FEFF}') {
        Some(stripped) => (stripped.to_string(), replaced),
        None => (text, replaced),
    }
}

/// Picks the body: native HTML if non-blank; else the HTML recovered from the RTF if there is
/// some; else the plain-text body if non-blank; else none.
pub(crate) fn select_body(forms: &BodyForms) -> Selection {
    let plain_text = forms.plain_text.clone().filter(|text| nonblank(text));
    let rtf_kind = forms.rtf.as_deref().map(recognize);

    if let Some(bytes) = &forms.native_html {
        let (html, undecodable) = decode_native_html(bytes, forms.native_html_code_page);
        if nonblank(&html) {
            return Selection {
                source: BodySource::NativeHtml,
                html: Some(html),
                plain_text,
                rtf_not_converted: false,
                rtf_kind,
                rtf_decompression_failed: forms.rtf_decompression_failed,
                undecodable_bytes: undecodable,
            };
        }
    }

    if let Some(rtf) = &forms.rtf
        && let Ok(recovered) = deencapsulate_html(rtf, Limits::default())
        && nonblank(&recovered.html)
    {
        return Selection {
            source: BodySource::RtfHtml,
            html: Some(recovered.html),
            plain_text,
            rtf_not_converted: false,
            rtf_kind,
            rtf_decompression_failed: forms.rtf_decompression_failed,
            undecodable_bytes: recovered.diagnostics.undecodable_bytes,
        };
    }

    Selection {
        source: if plain_text.is_some() {
            BodySource::PlainText
        } else {
            BodySource::None
        },
        html: None,
        plain_text,
        rtf_not_converted: forms.rtf.is_some(),
        rtf_kind,
        rtf_decompression_failed: forms.rtf_decompression_failed,
        undecodable_bytes: 0,
    }
}

// =============================================================================
// Preprocessing
// =============================================================================

/// Which preprocessing steps changed the HTML. Recorded in `metadata.json` by label.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) comments: bool,
    pub(crate) style: bool,
    pub(crate) script: bool,
    pub(crate) title: bool,
    pub(crate) downlevel_markers: bool,
    pub(crate) office_paragraph_tags: bool,
}

impl Applied {
    /// The labels of the steps that applied, in the closed list's order; `invisible_padding`
    /// (a step on the Markdown, not the HTML) is added when `padding` is true.
    pub(crate) fn labels(&self, padding: bool) -> Vec<&'static str> {
        let mut labels = Vec::new();
        for (applied, label) in [
            (self.comments, "comments"),
            (self.style, "style"),
            (self.script, "script"),
            (self.title, "title"),
            (self.downlevel_markers, "downlevel_markers"),
            (self.office_paragraph_tags, "office_paragraph_tags"),
            (padding, "invisible_padding"),
        ] {
            if applied {
                labels.push(label);
            }
        }
        labels
    }
}

/// True for the characters that may follow a tag name: the end of the tag, a self-closing slash,
/// or white space before attributes.
fn is_tag_delimiter(c: char) -> bool {
    c == '>' || c == '/' || c.is_whitespace()
}

/// `style`, `script`, or `title` when `lower_tail` (lowercased, just past a `<`) starts that
/// element's opening tag. `<styled>`, `<scripted>`, and `<titled>` are other elements.
fn raw_text_element_at(lower_tail: &str) -> Option<&'static str> {
    ["style", "script", "title"].into_iter().find(|name| {
        lower_tail
            .strip_prefix(*name)
            .and_then(|after| after.chars().next())
            .is_some_and(is_tag_delimiter)
    })
}

/// Where such an element ends: the byte just past the `>` of its closing tag, searching `lower`
/// from `from`. `None` when it is never closed.
fn end_of_raw_text_element(lower: &str, from: usize, name: &str) -> Option<usize> {
    let closing = format!("</{name}");
    let mut search = from;
    while let Some(relative) = lower[search..].find(&closing) {
        let after = search + relative + closing.len();
        if lower[after..].chars().next().is_none_or(is_tag_delimiter) {
            return Some(
                lower[after..]
                    .find('>')
                    .map_or(lower.len(), |q| after + q + 1),
            );
        }
        search = after;
    }
    None
}

fn note_raw_text_element(applied: &mut Applied, name: &str) {
    match name {
        "style" => applied.style = true,
        "script" => applied.script = true,
        _ => applied.title = true,
    }
}

/// Removes comments and `style`, `script`, and `title` elements (with their text) in one
/// left-to-right pass, so that whichever starts first in the document wins, as it would for a
/// browser: a `<!--` inside a script is script text, and a `<style>` inside a comment is comment
/// text. Anything never closed removes the rest of the document, as the HTML rules treat an
/// unterminated raw-text element.
fn strip_comments_style_script_title(html: &str, applied: &mut Applied) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        let Some(relative) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + relative;
        out.push_str(&html[i..lt]);
        if html[lt..].starts_with("<!--") {
            applied.comments = true;
            match html[lt + 4..].find("-->") {
                Some(end) => i = lt + 4 + end + 3,
                None => return out,
            }
        } else if let Some(name) = raw_text_element_at(&lower[lt + 1..]) {
            note_raw_text_element(applied, name);
            match end_of_raw_text_element(&lower, lt + 1 + name.len(), name) {
                Some(end) => i = end,
                None => return out,
            }
        } else {
            out.push('<');
            i = lt + 1;
        }
    }
    out
}

/// Removes `<![if ...]>` and `<![endif]>` markers (Word's downlevel-revealed sections).
fn strip_marked_sections(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<![") {
        out.push_str(&rest[..start]);
        match rest[start..].find("]>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Removes `<o:p>` and `</o:p>` tags, keeping what is inside.
fn remove_office_paragraph_tags(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        let Some(relative) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + relative;
        out.push_str(&html[i..lt]);
        let tail = &lower[lt + 1..];
        let after_slash = tail.strip_prefix('/').unwrap_or(tail);
        let is_office_p = after_slash.starts_with("o:p")
            && after_slash[3..]
                .chars()
                .next()
                .is_some_and(is_tag_delimiter);
        if is_office_p {
            i = html[lt..].find('>').map_or(html.len(), |end| lt + end + 1);
        } else {
            out.push('<');
            i = lt + 1;
        }
    }
    out
}

/// The preprocessing before conversion, and which steps changed the HTML.
pub(crate) fn preprocess_html_detailed(html: &str) -> (String, Applied) {
    let mut applied = Applied::default();
    let stripped = strip_comments_style_script_title(html, &mut applied);
    let unmarked = strip_marked_sections(&stripped);
    applied.downlevel_markers = unmarked.len() != stripped.len();
    let cleaned = remove_office_paragraph_tags(&unmarked);
    applied.office_paragraph_tags = cleaned.len() != unmarked.len();
    (cleaned, applied)
}

/// The preprocessing before conversion (the bake-off harness calls this too).
#[allow(dead_code)] // called by the bake-off harness, which not every build compiles
pub(crate) fn preprocess_html(html: &str) -> String {
    preprocess_html_detailed(html).0
}

// =============================================================================
// Invisible padding
// =============================================================================

/// The invisible formatting characters that newsletter templates use as padding. The direction
/// marks U+200E and U+200F are not in the set: they carry meaning.
fn is_invisible_padding_char(c: char) -> bool {
    matches!(
        c,
        '\u{034F}' | '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
    )
}

fn is_padding_space(c: char) -> bool {
    c == ' ' || c == '\u{00A0}'
}

/// Removes the padding runs of one line: each maximal run of invisible characters and spaces
/// that holds at least [`PADDING_MIN_RUN`] invisible characters is removed, leaving one space if
/// text continues on both sides. Returns the line and the number of runs removed.
fn remove_padding_runs(line: &str) -> (String, u64) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut removed = 0u64;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if !(is_invisible_padding_char(c) || is_padding_space(c)) {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        let mut invisible = 0usize;
        while j < chars.len() && (is_invisible_padding_char(chars[j]) || is_padding_space(chars[j]))
        {
            if is_invisible_padding_char(chars[j]) {
                invisible += 1;
            }
            j += 1;
        }
        if invisible >= PADDING_MIN_RUN {
            removed += 1;
            let text_before = out.chars().next_back().is_some_and(|p| !p.is_whitespace());
            let text_after = chars.get(j).is_some_and(|n| !n.is_whitespace());
            if text_before && text_after {
                out.push(' ');
            }
        } else {
            out.extend(&chars[start..j]);
        }
        i = j;
    }
    (out, removed)
}

/// Removes invisible padding runs from converted Markdown (ADR 2, decision 5). A line that held
/// only padding is removed, and blank lines left behind next to a removed line collapse to one.
/// Returns the text and the number of runs removed. Everything else is left exactly as it was.
pub(crate) fn collapse_invisible_padding(text: &str) -> (String, u64) {
    let mut runs = 0u64;
    let mut lines: Vec<String> = Vec::new();
    let mut dropped_pending = false;
    for line in text.split('\n') {
        let (clean, removed) = remove_padding_runs(line);
        if removed > 0 {
            runs += removed;
            if clean.trim().is_empty() {
                dropped_pending = true;
                continue;
            }
            dropped_pending = false;
            lines.push(clean);
            continue;
        }
        if line.trim().is_empty() {
            if dropped_pending && lines.last().is_none_or(|last| last.trim().is_empty()) {
                continue;
            }
        } else {
            dropped_pending = false;
        }
        lines.push(line.to_string());
    }
    (lines.join("\n"), runs)
}

// =============================================================================
// Conversion
// =============================================================================

/// The converter failed, panicked, or could not start. Recorded as a loss, never raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversionFailed;

/// `htmd` on a worker thread with a large stack. A panic is the thread's failure, not the
/// process's, and its message (which could quote content) is not printed.
pub(crate) fn convert_html_isolated(html: &str) -> Result<String, ConversionFailed> {
    let owned = html.to_string();
    let _guard = HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let spawned = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || htmd::convert(&owned).map_err(|_| ConversionFailed));
    let joined = match spawned {
        Ok(handle) => handle.join().map_err(|_| ConversionFailed),
        Err(_) => Err(ConversionFailed),
    };
    std::panic::set_hook(previous_hook);
    joined.and_then(|converted| converted)
}

fn count_html_rows(html: &str) -> u64 {
    let lower = html.to_ascii_lowercase();
    let mut count = 0u64;
    let mut from = 0usize;
    while let Some(relative) = lower[from..].find("<tr") {
        let after = from + relative + 3;
        if lower[after..].chars().next().is_some_and(is_tag_delimiter) {
            count += 1;
        }
        from = after;
    }
    count
}

fn count_markdown_table_rows(markdown: &str) -> u64 {
    markdown
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('|') && !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
        })
        .count() as u64
}

/// The `cid:` link destinations in converted Markdown: how many, and how many distinct (the
/// content ID compared without regard to case). Only a destination (`](cid:...)` or
/// `](<cid:...>)`) counts, so ordinary text such as "acid:" does not.
pub(crate) fn count_cid_references(markdown: &str) -> (u64, u64) {
    let references = find_cid_references(markdown);
    let distinct: BTreeSet<String> = references
        .iter()
        .map(|reference| reference.id.to_ascii_lowercase())
        .collect();
    (references.len() as u64, distinct.len() as u64)
}

/// One `cid:` link destination in converted Markdown, located exactly (M4f-2b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CidReference {
    /// Byte offset where the destination starts: at the `<` of an angle-bracket destination,
    /// otherwise at the `c` of `cid:`.
    pub(crate) start: usize,
    /// Byte offset just past the destination: past the content ID, or past the closing `>` of an
    /// angle-bracket destination. A title after the destination is not part of it.
    pub(crate) end: usize,
    /// The content ID as written (without `cid:` and without the angle brackets).
    pub(crate) id: String,
}

/// Every `cid:` link destination in `markdown`, in order, by the rule
/// [`count_cid_references`] has always used (it now counts these): the text after `](`, with an
/// optional `<`, starting with `cid:` in any case; the content ID ends at `)`, `>`, or white
/// space. An empty ID is still a reference.
pub(crate) fn find_cid_references(markdown: &str) -> Vec<CidReference> {
    let mut found = Vec::new();
    let mut offset = 0usize;
    let mut rest = markdown;
    while let Some(at) = rest.find("](") {
        let after_start = offset + at + 2;
        let after = &rest[at + 2..];
        let angle = after.starts_with('<');
        let destination = after.strip_prefix('<').unwrap_or(after);
        if let Some(prefix) = destination.get(..4)
            && prefix.eq_ignore_ascii_case("cid:")
        {
            let id_len: usize = destination[4..]
                .chars()
                .take_while(|c| !matches!(c, ')' | '>') && !c.is_whitespace())
                .map(char::len_utf8)
                .sum();
            let id_start = after_start + usize::from(angle) + 4;
            let mut end = id_start + id_len;
            if angle && markdown[end..].starts_with('>') {
                end += 1;
            }
            found.push(CidReference {
                start: after_start,
                end,
                id: markdown[id_start..id_start + id_len].to_string(),
            });
        }
        offset = after_start;
        rest = after;
    }
    found
}

/// Decodes `%XX` escapes; text that is not valid UTF-8 afterwards is returned unchanged, and a
/// `%` not followed by two hex digits stays a `%`.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(i + 1).and_then(|b| char::from(*b).to_digit(16)),
                bytes.get(i + 2).and_then(|b| char::from(*b).to_digit(16)),
            )
        {
            out.push((high * 16 + low) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// What a content ID is compared by: white space and one pair of angle brackets removed, `%XX`
/// escapes decoded, ASCII letters in lower case. The same function is applied to the ID in a
/// `cid:` link and to `PidTagAttachContentId`, so `<Logo@x>` and `logo%40x` are one ID.
pub(crate) fn content_id_key(id: &str) -> String {
    let trimmed = id.trim();
    let unwrapped = trimmed
        .strip_prefix('<')
        .and_then(|text| text.strip_suffix('>'))
        .unwrap_or(trimmed);
    percent_decode(unwrapped).to_ascii_lowercase()
}

/// The link from a `message.md` to an extracted attachment of its own message: relative, with
/// every byte outside `A-Z a-z 0-9 - . _ ~` percent-encoded (so a space, a parenthesis, or a
/// non-ASCII letter cannot end or break the Markdown destination). A nested message's
/// `message.md` sits beside its own `attachments/`, so the same form serves every level.
pub(crate) fn attachment_link(file_name: &str) -> String {
    let mut link = format!("{}/", crate::layout::ATTACHMENTS_DIR);
    for byte in file_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            link.push(char::from(byte));
        } else {
            link.push_str(&format!("%{byte:02X}"));
        }
    }
    link
}

/// `text` with each `(start, end, replacement)` range replaced. The ranges must be in order and
/// must not overlap.
pub(crate) fn replace_ranges(text: &str, replacements: &[(usize, usize, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for (start, end, replacement) in replacements {
        out.push_str(&text[at..*start]);
        out.push_str(replacement);
        at = *end;
    }
    out.push_str(&text[at..]);
    out
}

// =============================================================================
// Rendering and loss
// =============================================================================

/// The closed vocabulary of formatting-loss reasons (draft schema `0.1-draft`; a new reason is a
/// schema change). The first four are decided by ADR 2; `HtmlConversionFailed` records a
/// conversion that failed and fell back to the plain-text body; `RtfDecompressionFailed` records
/// an RTF property that existed and could not be decompressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LossReason {
    /// Table rows in the HTML that have no Markdown table row.
    TablesFlattened,
    /// An RTF existed that could not be used.
    RtfNotConverted,
    /// Bytes replaced by U+FFFD because they could not be decoded.
    UndecodableBytesReplaced,
    /// Runs of invisible padding removed from the Markdown.
    InvisiblePaddingRemoved,
    /// The converter failed; the plain-text body (if any) was used instead.
    HtmlConversionFailed,
    /// A non-empty RTF property existed and could not be decompressed.
    RtfDecompressionFailed,
}

impl LossReason {
    /// Every reason, in the vocabulary's order (the order `loss` is written in).
    pub(crate) const ALL: [LossReason; 6] = [
        LossReason::TablesFlattened,
        LossReason::RtfNotConverted,
        LossReason::UndecodableBytesReplaced,
        LossReason::InvisiblePaddingRemoved,
        LossReason::HtmlConversionFailed,
        LossReason::RtfDecompressionFailed,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            LossReason::TablesFlattened => "tables_flattened",
            LossReason::RtfNotConverted => "rtf_not_converted",
            LossReason::UndecodableBytesReplaced => "undecodable_bytes_replaced",
            LossReason::InvisiblePaddingRemoved => "invisible_padding_removed",
            LossReason::HtmlConversionFailed => "html_conversion_failed",
            LossReason::RtfDecompressionFailed => "rtf_decompression_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Loss {
    pub(crate) reason: LossReason,
    pub(crate) count: u64,
}

/// What `message.md` and `metadata.json` need from the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rendered {
    /// The source actually used (a failed conversion falls back to plain text or none).
    pub(crate) source: BodySource,
    /// The converted Markdown, for the HTML sources.
    pub(crate) markdown: Option<String>,
    /// The plain-text body verbatim, when it is the source used (the caller fences it).
    pub(crate) plain_text: Option<String>,
    /// The preprocessing steps that changed something, from the closed list, in its order.
    pub(crate) preprocessing: Vec<&'static str>,
    /// Loss, in the vocabulary's order, only for reasons with a count above zero.
    pub(crate) loss: Vec<Loss>,
}

fn push_loss(loss: &mut Vec<Loss>, reason: LossReason, count: u64) {
    if count > 0 {
        loss.push(Loss { reason, count });
    }
}

/// Renders the selected body with the production converter.
#[cfg(test)]
pub(crate) fn render_body(selection: &Selection) -> Rendered {
    render_with(selection, &convert_html_isolated)
}

/// [`render_body`] with the converter supplied, so failure and loss paths can be tested. The
/// converter is given the preprocessed HTML.
pub(crate) fn render_with(
    selection: &Selection,
    convert: &dyn Fn(&str) -> Result<String, ConversionFailed>,
) -> Rendered {
    let rtf_decompression_failed = u64::from(selection.rtf_decompression_failed);
    let Some(html) = &selection.html else {
        // Plain text or no body at all.
        let mut loss = Vec::new();
        push_loss(
            &mut loss,
            LossReason::RtfNotConverted,
            u64::from(selection.rtf_not_converted),
        );
        push_loss(
            &mut loss,
            LossReason::RtfDecompressionFailed,
            rtf_decompression_failed,
        );
        return Rendered {
            source: selection.source,
            markdown: None,
            plain_text: match selection.source {
                BodySource::PlainText => selection.plain_text.clone(),
                _ => None,
            },
            preprocessing: Vec::new(),
            loss,
        };
    };

    let (preprocessed, applied) = preprocess_html_detailed(html);
    let Ok(converted) = convert(&preprocessed) else {
        // The conversion failed: use the plain-text body if there is one, and say so.
        let mut loss = Vec::new();
        push_loss(&mut loss, LossReason::HtmlConversionFailed, 1);
        push_loss(
            &mut loss,
            LossReason::RtfDecompressionFailed,
            rtf_decompression_failed,
        );
        let plain_text = selection.plain_text.clone();
        return Rendered {
            source: if plain_text.is_some() {
                BodySource::PlainText
            } else {
                BodySource::None
            },
            markdown: None,
            plain_text,
            preprocessing: applied.labels(false),
            loss,
        };
    };

    let (markdown, padding_runs) = collapse_invisible_padding(&converted);
    let rows_lost =
        count_html_rows(&preprocessed).saturating_sub(count_markdown_table_rows(&markdown));
    let mut loss = Vec::new();
    push_loss(&mut loss, LossReason::TablesFlattened, rows_lost);
    push_loss(
        &mut loss,
        LossReason::UndecodableBytesReplaced,
        selection.undecodable_bytes,
    );
    push_loss(&mut loss, LossReason::InvisiblePaddingRemoved, padding_runs);
    push_loss(
        &mut loss,
        LossReason::RtfDecompressionFailed,
        rtf_decompression_failed,
    );
    Rendered {
        source: selection.source,
        markdown: Some(markdown),
        plain_text: None,
        preprocessing: applied.labels(padding_runs > 0),
        loss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCAPSULATED: &[u8] = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl{\f0\fswiss\fcharset0 Arial;}}{\*\htmltag64}{\*\htmltag84 <b>}Hello{\*\htmltag92 </b>}}";
    const PLAIN_RTF: &[u8] = br"{\rtf1\ansi Hello}";

    fn forms() -> BodyForms {
        BodyForms::default()
    }

    fn ok(markdown: &'static str) -> impl Fn(&str) -> Result<String, ConversionFailed> {
        move |_| Ok(markdown.to_string())
    }

    // ----- preprocessing -----

    #[test]
    fn preprocessing_removes_word_comments_markers_and_office_paragraph_tags() {
        let html = "a<!--[if gte mso 9]><xml>x</xml><![endif]-->b<![if !supportLists]>c<![endif]>d<o:p>e</o:p>f<O:P/>g<p>h</p>";
        assert_eq!(preprocess_html(html), "abcdefg<p>h</p>");
        // Something that only looks like it: left alone.
        assert_eq!(preprocess_html("<o:pen>x</o:pen>"), "<o:pen>x</o:pen>");
        // An unterminated comment drops the rest rather than looping.
        assert_eq!(preprocess_html("a<!-- never closed"), "a");
    }

    #[test]
    fn preprocessing_removes_style_and_script_elements_with_their_text() {
        let html = "<p>a</p><style type=\"text/css\">p{color:red}</style><p>b</p><SCRIPT>var x = \"<b>t</b>\";</SCRIPT><p>c</p>";
        assert_eq!(preprocess_html(html), "<p>a</p><p>b</p><p>c</p>");
    }

    #[test]
    fn preprocessing_removes_title_elements_with_their_text() {
        let html =
            "<html><head><TITLE lang=en>The Title</TITLE></head><body><p>Body</p></body></html>";
        assert_eq!(
            preprocess_html(html),
            "<html><head></head><body><p>Body</p></body></html>"
        );
        // An unterminated title is title text to the end of the document.
        assert_eq!(preprocess_html("a<title>never closed <p>x</p>"), "a");
    }

    #[test]
    fn preprocessing_leaves_elements_that_only_start_with_a_removed_name() {
        let html = "<styled>x</styled><scripted>y</scripted><titled>z</titled><p>w</p>";
        assert_eq!(preprocess_html(html), html);
    }

    #[test]
    fn preprocessing_reads_comments_and_raw_text_in_document_order() {
        // Word wraps its CSS in a comment inside the style element, and hides more style
        // elements in conditional comments: both go.
        let word = "<head><style><!-- p.MsoNormal{margin:0} --></style><!--[if gte mso 9]><style>v|* {behavior:url(#default#VML);}</style><![endif]--></head><body>x</body>";
        assert_eq!(preprocess_html(word), "<head></head><body>x</body>");
        // A `<!--` inside a script is script text, so it does not swallow what follows.
        assert_eq!(
            preprocess_html("<script>var a = \"<!--\";</script><p>kept</p>"),
            "<p>kept</p>"
        );
        // A `<title>` inside a comment is comment text.
        assert_eq!(
            preprocess_html("<!-- <title> --><p>kept</p>"),
            "<p>kept</p>"
        );
    }

    #[test]
    fn an_unterminated_style_or_script_drops_the_rest() {
        assert_eq!(preprocess_html("a<style>p{}"), "a");
        assert_eq!(preprocess_html("a<script>var x;"), "a");
    }

    #[test]
    fn the_steps_that_applied_are_reported_in_the_closed_lists_order() {
        let html = "<!-- c --><title>t</title><style>s</style><script>j</script><![if x]><o:p></o:p><p>a</p>";
        let (text, applied) = preprocess_html_detailed(html);
        assert_eq!(text, "<p>a</p>");
        assert_eq!(
            applied.labels(true),
            vec![
                "comments",
                "style",
                "script",
                "title",
                "downlevel_markers",
                "office_paragraph_tags",
                "invisible_padding"
            ]
        );
        let (_, nothing) = preprocess_html_detailed("<p>plain</p>");
        assert!(nothing.labels(false).is_empty());
        assert_eq!(nothing.labels(true), vec!["invisible_padding"]);
    }

    // ----- native HTML decoding -----

    #[test]
    fn native_html_is_decoded_by_its_code_page() {
        assert_eq!(
            decode_native_html(b"caf\xe9", Some(1252)),
            ("caf\u{e9}".to_string(), 0)
        );
        assert_eq!(
            decode_native_html("caf\u{e9}".as_bytes(), None),
            ("caf\u{e9}".to_string(), 0)
        );
        assert_eq!(
            decode_native_html("caf\u{e9}".as_bytes(), Some(65001)),
            ("caf\u{e9}".to_string(), 0)
        );
    }

    #[test]
    fn bytes_that_cannot_be_decoded_are_replaced_and_counted() {
        // Invalid UTF-8.
        assert_eq!(
            decode_native_html(b"a\xffb", None),
            ("a\u{FFFD}b".to_string(), 1)
        );
        // A code page that is not implemented: non-ASCII bytes only.
        assert_eq!(
            decode_native_html(b"a\xefb\xef", Some(1251)),
            ("a\u{FFFD}b\u{FFFD}".to_string(), 2)
        );
        // ASCII under an unimplemented code page is fine.
        assert_eq!(
            decode_native_html(b"abc", Some(1251)),
            ("abc".to_string(), 0)
        );
    }

    #[test]
    fn a_byte_order_mark_is_dropped() {
        assert_eq!(
            decode_native_html(b"\xef\xbb\xbf<p>x</p>", None),
            ("<p>x</p>".to_string(), 0)
        );
    }

    // ----- selection -----

    #[test]
    fn native_html_wins_over_the_rtf_and_the_plain_text() {
        let selection = select_body(&BodyForms {
            native_html: Some(b"<p>x</p>".to_vec()),
            rtf: Some(ENCAPSULATED.to_vec()),
            plain_text: Some("p".to_string()),
            ..forms()
        });
        assert_eq!(selection.source, BodySource::NativeHtml);
        assert_eq!(selection.html.as_deref(), Some("<p>x</p>"));
        assert_eq!(selection.plain_text.as_deref(), Some("p"));
        assert!(!selection.rtf_not_converted);
        assert_eq!(selection.rtf_kind, Some(RtfKind::EncapsulatedHtml));
    }

    #[test]
    fn html_recovered_from_the_rtf_is_next() {
        let selection = select_body(&BodyForms {
            rtf: Some(ENCAPSULATED.to_vec()),
            plain_text: Some("Hello".to_string()),
            ..forms()
        });
        assert_eq!(selection.source, BodySource::RtfHtml);
        assert_eq!(selection.html.as_deref(), Some("<b>Hello</b>"));
        assert!(!selection.rtf_not_converted);
    }

    #[test]
    fn a_blank_native_html_falls_through_to_the_rtf() {
        let selection = select_body(&BodyForms {
            native_html: Some(b" \r\n ".to_vec()),
            rtf: Some(ENCAPSULATED.to_vec()),
            ..forms()
        });
        assert_eq!(selection.source, BodySource::RtfHtml);
    }

    #[test]
    fn an_rtf_without_encapsulated_html_is_flagged_and_the_plain_text_is_used() {
        let selection = select_body(&BodyForms {
            rtf: Some(PLAIN_RTF.to_vec()),
            plain_text: Some("Hello".to_string()),
            ..forms()
        });
        assert_eq!(selection.source, BodySource::PlainText);
        assert_eq!(selection.plain_text.as_deref(), Some("Hello"));
        assert!(selection.rtf_not_converted);
        assert_eq!(selection.rtf_kind, Some(RtfKind::PlainRtf));
        assert!(selection.html.is_none());
    }

    #[test]
    fn plain_text_alone_and_no_body_at_all() {
        let plain = select_body(&BodyForms {
            plain_text: Some("Hello".to_string()),
            ..forms()
        });
        assert_eq!(plain.source, BodySource::PlainText);
        assert!(!plain.rtf_not_converted);
        assert_eq!(plain.rtf_kind, None);

        let blank = select_body(&BodyForms {
            plain_text: Some(" \n".to_string()),
            ..forms()
        });
        assert_eq!(blank.source, BodySource::None);
        assert!(blank.plain_text.is_none());

        assert_eq!(select_body(&forms()).source, BodySource::None);
    }

    #[test]
    fn an_unusable_rtf_with_no_plain_text_is_flagged() {
        let selection = select_body(&BodyForms {
            rtf: Some(PLAIN_RTF.to_vec()),
            ..forms()
        });
        assert_eq!(selection.source, BodySource::None);
        assert!(selection.rtf_not_converted);
    }

    #[test]
    fn the_source_labels_are_the_recorded_ones() {
        assert_eq!(BodySource::NativeHtml.label(), "native_html");
        assert_eq!(BodySource::RtfHtml.label(), "rtf_html");
        assert_eq!(BodySource::PlainText.label(), "plain_text");
        assert_eq!(BodySource::None.label(), "none");
    }

    // ----- invisible padding -----

    fn padding(n: usize) -> String {
        "\u{034F} \u{00AD}".repeat(n)
    }

    #[test]
    fn a_long_padding_paragraph_is_removed_and_the_blank_lines_collapse() {
        let text = format!("Thank you\n\n{}\n\nWelcome", padding(200));
        let (clean, runs) = collapse_invisible_padding(&text);
        assert_eq!(clean, "Thank you\n\nWelcome");
        assert_eq!(runs, 1);
    }

    #[test]
    fn short_runs_and_direction_marks_stay() {
        // Seven invisible characters in a row.
        let seven = "\u{200B}".repeat(7);
        let text = format!("a{seven}b");
        assert_eq!(collapse_invisible_padding(&text), (text.clone(), 0));
        // The right-to-left and left-to-right marks are never padding.
        let marks = "\u{200F}".repeat(20);
        let text = format!("x {marks} y");
        assert_eq!(collapse_invisible_padding(&text), (text.clone(), 0));
        // An emoji joiner sequence.
        let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".to_string();
        assert_eq!(collapse_invisible_padding(&text), (text.clone(), 0));
    }

    #[test]
    fn padding_between_words_leaves_one_space() {
        let text = format!("before {} after", padding(10));
        assert_eq!(
            collapse_invisible_padding(&text),
            ("before after".to_string(), 1)
        );
        let glued = format!("before{}after", "\u{200B}".repeat(9));
        assert_eq!(
            collapse_invisible_padding(&glued),
            ("before after".to_string(), 1)
        );
    }

    #[test]
    fn padding_at_the_start_or_end_of_a_line_leaves_no_stray_space() {
        let text = format!("{}word", padding(10));
        assert_eq!(collapse_invisible_padding(&text), ("word".to_string(), 1));
        let text = format!("word{}", padding(10));
        assert_eq!(collapse_invisible_padding(&text), ("word".to_string(), 1));
    }

    #[test]
    fn a_padding_line_at_the_start_of_the_text_leaves_no_leading_blank_line() {
        let text = format!("{}\n\nText", padding(10));
        assert_eq!(collapse_invisible_padding(&text), ("Text".to_string(), 1));
    }

    #[test]
    fn text_without_padding_is_returned_exactly() {
        let text = "# Title\n\n\n\nBody  with  spaces\n\u{00A0}\n";
        assert_eq!(collapse_invisible_padding(text), (text.to_string(), 0));
    }

    #[test]
    fn each_run_is_counted_and_blank_lines_elsewhere_are_untouched() {
        let text = format!("a\n\n\n\n{}\n\nb {} c", padding(9), padding(9));
        let (clean, runs) = collapse_invisible_padding(&text);
        assert_eq!(runs, 2);
        assert!(clean.starts_with("a\n\n\n\n"));
        assert!(clean.ends_with("b c"));
    }

    // ----- counting -----

    #[test]
    fn rows_are_counted_in_the_html_and_the_markdown() {
        assert_eq!(
            count_html_rows("<table><tr><td>a</td></tr><TR class=x><td>b</td></tr></table>"),
            2
        );
        assert_eq!(count_html_rows("<track><trap>"), 0);
        let md = "| a | b |\n| --- | --- |\n| 1 | 2 |\ntext | not a row\n";
        assert_eq!(count_markdown_table_rows(md), 2);
    }

    #[test]
    fn cid_references_count_destinations_only() {
        let md =
            "![a](cid:one@x) and [b](<CID:ONE@x>) ![c](cid:two@x) [d](http://x) acid:zzz [e](cid:)";
        assert_eq!(count_cid_references(md), (4, 3));
        assert_eq!(count_cid_references("no links"), (0, 0));
        assert_eq!(count_cid_references("](cid:a) ](cid:a)"), (2, 1));
    }

    // ----- cid links (M4f-2b) -----

    #[test]
    fn cid_references_are_located_with_their_exact_extent() {
        let md = "![a](cid:one@x) [b](<CID:Two@x> \"t\") [c](http://x) acid:z ![d](cid:three@x \"title\")";
        let found = find_cid_references(md);
        let ids: Vec<&str> = found.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["one@x", "Two@x", "three@x"]);
        assert_eq!(&md[found[0].start..found[0].end], "cid:one@x");
        assert_eq!(&md[found[1].start..found[1].end], "<CID:Two@x>");
        assert_eq!(&md[found[2].start..found[2].end], "cid:three@x");
        // Counting and locating agree.
        assert_eq!(count_cid_references(md), (3, 3));
    }

    #[test]
    fn content_ids_compare_without_brackets_case_or_percent_escapes() {
        assert_eq!(
            content_id_key("<Image001.PNG@01D2.A1>"),
            "image001.png@01d2.a1"
        );
        assert_eq!(
            content_id_key(" image001.png%4001d2.a1 "),
            "image001.png@01d2.a1"
        );
        assert_eq!(content_id_key("100%"), "100%");
        assert_eq!(content_id_key("%FF"), "%ff");
        assert_eq!(content_id_key("<>"), "");
        assert_eq!(content_id_key(""), "");
    }

    #[test]
    fn attachment_links_are_relative_and_percent_encoded() {
        assert_eq!(attachment_link("logo.png"), "attachments/logo.png");
        assert_eq!(
            attachment_link("my file (1).png"),
            "attachments/my%20file%20%281%29.png"
        );
        assert_eq!(
            attachment_link("caf\u{e9}.png"),
            "attachments/caf%C3%A9.png"
        );
        assert_eq!(attachment_link("a_b-c~d.e"), "attachments/a_b-c~d.e");
    }

    #[test]
    fn ranges_are_replaced_in_order() {
        assert_eq!(
            replace_ranges(
                "0123456789",
                &[(1, 3, "A".to_string()), (5, 9, "BB".to_string())]
            ),
            "0A34BB9"
        );
        assert_eq!(replace_ranges("abc", &[]), "abc");
    }

    // ----- rendering -----

    #[test]
    fn a_plain_text_body_is_returned_verbatim_for_the_caller_to_fence() {
        let selection = select_body(&BodyForms {
            plain_text: Some("Hello\r\nWorld".to_string()),
            ..forms()
        });
        let rendered = render_with(&selection, &ok("unused"));
        assert_eq!(rendered.source, BodySource::PlainText);
        assert_eq!(rendered.plain_text.as_deref(), Some("Hello\r\nWorld"));
        assert!(rendered.markdown.is_none());
        assert!(rendered.preprocessing.is_empty());
        assert!(rendered.loss.is_empty());
    }

    #[test]
    fn an_unusable_rtf_is_recorded_as_loss() {
        let selection = select_body(&BodyForms {
            rtf: Some(PLAIN_RTF.to_vec()),
            plain_text: Some("Hello".to_string()),
            ..forms()
        });
        let rendered = render_with(&selection, &ok("unused"));
        assert_eq!(
            rendered.loss,
            vec![Loss {
                reason: LossReason::RtfNotConverted,
                count: 1
            }]
        );
    }

    #[test]
    fn an_rtf_that_could_not_be_decompressed_is_recorded_on_every_path() {
        let failed = BodyForms {
            rtf_decompression_failed: true,
            plain_text: Some("x".to_string()),
            ..forms()
        };
        let plain = render_with(&select_body(&failed), &ok("unused"));
        assert_eq!(
            plain.loss,
            vec![Loss {
                reason: LossReason::RtfDecompressionFailed,
                count: 1
            }]
        );
        let with_html = BodyForms {
            native_html: Some(b"<p>x</p>".to_vec()),
            rtf_decompression_failed: true,
            ..forms()
        };
        let converted = render_with(&select_body(&with_html), &ok("x"));
        assert_eq!(
            converted.loss,
            vec![Loss {
                reason: LossReason::RtfDecompressionFailed,
                count: 1
            }]
        );
        let broken = render_with(&select_body(&with_html), &|_| Err(ConversionFailed));
        assert_eq!(
            broken.loss,
            vec![
                Loss {
                    reason: LossReason::HtmlConversionFailed,
                    count: 1
                },
                Loss {
                    reason: LossReason::RtfDecompressionFailed,
                    count: 1
                }
            ]
        );
    }

    #[test]
    fn no_body_renders_as_nothing() {
        let rendered = render_with(&select_body(&forms()), &ok("unused"));
        assert_eq!(rendered.source, BodySource::None);
        assert!(rendered.markdown.is_none() && rendered.plain_text.is_none());
        assert!(rendered.loss.is_empty());
    }

    #[test]
    fn an_html_body_is_preprocessed_converted_and_its_loss_recorded() {
        let html = "<style>p{}</style><table><tr><td>a</td></tr><tr><td>b</td></tr><tr><td>c</td></tr></table>";
        let selection = select_body(&BodyForms {
            native_html: Some(html.as_bytes().to_vec()),
            ..forms()
        });
        // The stand-in converter must receive the preprocessed HTML.
        let seen = |input: &str| -> Result<String, ConversionFailed> {
            assert!(!input.contains("<style"));
            Ok("text".to_string())
        };
        let rendered = render_with(&selection, &seen);
        assert_eq!(rendered.source, BodySource::NativeHtml);
        assert_eq!(rendered.markdown.as_deref(), Some("text"));
        assert_eq!(rendered.preprocessing, vec!["style"]);
        assert_eq!(
            rendered.loss,
            vec![Loss {
                reason: LossReason::TablesFlattened,
                count: 3
            }]
        );
    }

    #[test]
    fn padding_and_undecodable_bytes_are_recorded_in_the_vocabulary_order() {
        let selection = select_body(&BodyForms {
            native_html: Some(b"<p>a\xff</p>".to_vec()),
            ..forms()
        });
        let converted = format!("a\n\n{}\n\nb", padding(20));
        let rendered = render_with(&selection, &|_| Ok(converted.clone()));
        assert_eq!(rendered.markdown.as_deref(), Some("a\n\nb"));
        assert_eq!(rendered.preprocessing, vec!["invisible_padding"]);
        assert_eq!(
            rendered.loss,
            vec![
                Loss {
                    reason: LossReason::UndecodableBytesReplaced,
                    count: 1
                },
                Loss {
                    reason: LossReason::InvisiblePaddingRemoved,
                    count: 1
                }
            ]
        );
    }

    #[test]
    fn a_failed_conversion_falls_back_to_the_plain_text_and_says_so() {
        let selection = select_body(&BodyForms {
            native_html: Some(b"<p>x</p>".to_vec()),
            plain_text: Some("fallback".to_string()),
            ..forms()
        });
        let rendered = render_with(&selection, &|_| Err(ConversionFailed));
        assert_eq!(rendered.source, BodySource::PlainText);
        assert_eq!(rendered.plain_text.as_deref(), Some("fallback"));
        assert!(rendered.markdown.is_none());
        assert_eq!(
            rendered.loss,
            vec![Loss {
                reason: LossReason::HtmlConversionFailed,
                count: 1
            }]
        );

        let without = select_body(&BodyForms {
            native_html: Some(b"<p>x</p>".to_vec()),
            ..forms()
        });
        let rendered = render_with(&without, &|_| Err(ConversionFailed));
        assert_eq!(rendered.source, BodySource::None);
        assert!(rendered.plain_text.is_none());
    }

    #[test]
    fn the_loss_labels_are_the_recorded_ones_in_the_vocabulary_order() {
        let labels: Vec<&str> = LossReason::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(
            labels,
            vec![
                "tables_flattened",
                "rtf_not_converted",
                "undecodable_bytes_replaced",
                "invisible_padding_removed",
                "html_conversion_failed",
                "rtf_decompression_failed"
            ]
        );
    }

    // ----- the real converter -----

    #[test]
    fn htmd_converts_a_simple_body_on_the_isolated_thread() {
        let markdown = convert_html_isolated("<p>Hello <b>world</b></p>").unwrap();
        assert_eq!(markdown.trim(), "Hello **world**");
    }

    #[test]
    fn the_title_never_reaches_the_markdown() {
        let selection = select_body(&BodyForms {
            native_html: Some(
                b"<html><head><title>Subject Line Title</title></head><body><p>Body text</p></body></html>"
                    .to_vec(),
            ),
            ..forms()
        });
        let rendered = render_body(&selection);
        let markdown = rendered.markdown.unwrap();
        assert!(markdown.contains("Body text"));
        assert!(!markdown.contains("Subject Line Title"));
        assert_eq!(rendered.preprocessing, vec!["title"]);
    }

    #[test]
    fn an_encapsulated_rtf_renders_to_markdown_end_to_end() {
        let selection = select_body(&BodyForms {
            rtf: Some(ENCAPSULATED.to_vec()),
            ..forms()
        });
        let rendered = render_body(&selection);
        assert_eq!(rendered.source, BodySource::RtfHtml);
        assert_eq!(
            rendered.markdown.as_deref().map(str::trim),
            Some("**Hello**")
        );
        assert!(rendered.loss.is_empty());
    }

    #[test]
    fn deep_nesting_converts_without_overflowing_the_stack() {
        let html = format!("{}deep{}", "<div>".repeat(300), "</div>".repeat(300));
        let markdown = convert_html_isolated(&html).unwrap();
        assert!(markdown.contains("deep"));
    }
}
