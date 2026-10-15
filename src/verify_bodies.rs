//! Content-preservation check (`--verify-bodies`, M4e-2c): does the Markdown the export writes
//! keep every visible word of the HTML it was converted from, in order? Decided by the body ADR
//! (decision 6); the exclusions below are the ones that ADR lists plus three that the
//! implementation needed and that are stated here so they can be overruled.
//!
//! For each message whose selected body is HTML (native, or recovered from the RTF), the check
//! runs the production path (`select_body`, the preprocessing, `htmd`, the padding rule: the same
//! calls the export makes) and compares two word lists:
//!
//! - **HTML words.** The text outside tags, after the production preprocessing (which removes
//!   comments and `style`, `script`, and `title` elements) and without the content of a closed
//!   `<head>`. Tags and their attributes are never words, so image alt text and link
//!   destinations are excluded. Entities are decoded (a named entity outside the built-in table
//!   is counted as `html_unresolved_entities_total`, not guessed). Block-level elements and `br`
//!   separate words; inline elements join them (`month</span><span>¹</span>` is one word, as
//!   `htmd` writes it).
//! - **Markdown words.** The Markdown the export writes, with link destinations, image alt text
//!   and image destinations dropped (link text kept), backslash escapes undone, and angle-bracket
//!   autolinks unwrapped.
//! - **Both sides.** The characters `*`, `_`, `~`, and the backtick (emphasis and code marks) and
//!   the invisible padding characters (U+034F, U+00AD, U+200B, U+200C, U+200D, U+2060, U+FEFF)
//!   are removed from every word, and a word left empty is dropped. (The ADR names only the
//!   removed padding; the emphasis marks are the same kind of syntax and `htmd` writes them.)
//! - **Markdown syntax words** (a run of `#`, `>`, `-`, `+`, `|`, `:`, or `=`, and an ordered-list
//!   numeral such as `1.` or `2)`) are *excused* when they sit in the Markdown where the HTML has
//!   a different word; a syntax word that equals the HTML's next word simply matches. (The ADR
//!   excludes list numerals unconditionally; excusing only on a mismatch cannot hide a lost
//!   literal `-` or `1.` that the HTML really contained.)
//!
//! The comparison is **greedy and in order**: when the next words differ, each side is searched
//! up to [`RESYNC_WINDOW`] words ahead for the word the other side is at, and the shorter skip is
//! taken as a run of missing (HTML-only) or extra (Markdown-only) words. It is not a minimal edit
//! script, so a non-zero count is approximate; zero is exact (zero missing and zero extra words
//! means the two lists are equal once syntax words are excused).
//!
//! Like the other verify modes this reads real content internally and prints only counts, keyed
//! by the message's position in the scanned list (the same numbering `--dump-deencap` uses). With
//! `--dump-bodies <dir>` it also writes, for each message with a difference, the two normalized
//! word lists (one word per line) into the directory the user names, so they can be compared
//! locally with any diff tool. That is the only place content goes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::archive::body_outcome;
use crate::body::{BodySource, preprocess_html_detailed, select_body};
use crate::source_msg::read_message_content;

/// How far ahead each side is searched for the word the other side is at.
const RESYNC_WINDOW: usize = 64;

/// Elements whose boundaries separate words. Every other element is inline: its tags add no
/// white space, so text on either side of it is one word.
const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "br",
    "caption",
    "center",
    "dd",
    "details",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "html",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// The named entities of U+00A0 to U+00FF, in code point order (index 0 is U+00A0).
const LATIN1_NAMES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
    "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute",
    "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute",
    "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute",
    "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil",
    "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc",
    "uuml", "yacute", "thorn", "yuml",
];

/// What the numeric references U+0080 to U+009F mean in HTML (the Windows-1252 table).
const C1_REPLACEMENTS: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

// =============================================================================
// Words
// =============================================================================

/// Characters removed from every word on both sides: emphasis and code marks, and the invisible
/// padding characters.
fn is_stripped(c: char) -> bool {
    matches!(
        c,
        '*' | '_'
            | '~'
            | '`'
            | '\u{034F}'
            | '\u{00AD}'
            | '\u{200B}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{2060}'
            | '\u{FEFF}'
    )
}

/// Splits on Unicode white space (which includes the no-break space), removes the stripped
/// characters from each word, and drops words left empty.
fn to_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| !is_stripped(*c))
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// A word that is Markdown syntax rather than text: a run of `#`, `>`, `-`, `+`, `|`, `:`, or
/// `=`, or an ordered-list numeral (one to nine digits and a `.` or `)`).
fn is_markdown_syntax_word(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    if word
        .chars()
        .all(|c| matches!(c, '#' | '>' | '-' | '+' | '|' | ':' | '='))
    {
        return true;
    }
    let digits = word
        .strip_suffix('.')
        .or_else(|| word.strip_suffix(')'))
        .unwrap_or("");
    !digits.is_empty() && digits.len() <= 9 && digits.chars().all(|c| c.is_ascii_digit())
}

// ----- HTML side -------------------------------------------------------------

fn is_block_tag(name: &str) -> bool {
    BLOCK_TAGS.contains(&name)
}

/// The tag name (lower case) and whether it is a closing tag, for the `<` at `lt`. `None` when
/// what follows the `<` cannot start a tag (HTML treats such a `<` as text).
fn tag_name(chars: &[char], lt: usize) -> Option<(String, bool)> {
    let mut i = lt + 1;
    let closing = chars.get(i) == Some(&'/');
    if closing {
        i += 1;
    }
    let first = *chars.get(i)?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut name = String::new();
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.') {
            name.push(c.to_ascii_lowercase());
            i += 1;
        } else {
            break;
        }
    }
    Some((name, closing))
}

/// The index just past the `>` that ends the tag starting at `lt`. A quote opens a quoted
/// attribute value only right after `=`, so an apostrophe in an unquoted value does not swallow
/// the rest of the document. An unterminated tag runs to the end.
fn tag_end(chars: &[char], lt: usize) -> usize {
    let mut i = lt + 1;
    let mut quote: Option<char> = None;
    let mut previous_significant = '<';
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(open) => {
                if c == open {
                    quote = None;
                    previous_significant = c;
                }
            }
            None => {
                if (c == '"' || c == '\'') && previous_significant == '=' {
                    quote = Some(c);
                } else if c == '>' {
                    return i + 1;
                }
                if !c.is_whitespace() {
                    previous_significant = c;
                }
            }
        }
        i += 1;
    }
    chars.len()
}

/// The end of the `</head>` tag that closes a head whose opening tag ended at `from`, or `None`
/// when the head is never closed (then its content is kept: an unterminated head must not hide
/// the body).
fn head_close_end(chars: &[char], lower: &[char], from: usize) -> Option<usize> {
    let needle: Vec<char> = "</head".chars().collect();
    let mut k = from;
    while k + needle.len() <= lower.len() {
        if lower[k..k + needle.len()] == needle[..] {
            let after = lower.get(k + needle.len()).copied();
            if after.is_none_or(|c| c == '>' || c == '/' || c.is_whitespace()) {
                return Some(tag_end(chars, k));
            }
        }
        k += 1;
    }
    None
}

enum Entity {
    Char(char),
    Unresolved,
}

fn numeric_reference(value: u32) -> char {
    match value {
        0 => '\u{FFFD}',
        0x80..=0x9F => C1_REPLACEMENTS[(value - 0x80) as usize],
        _ => char::from_u32(value).unwrap_or('\u{FFFD}'),
    }
}

fn named_entity(name: &str) -> Option<char> {
    if let Some(position) = LATIN1_NAMES.iter().position(|candidate| *candidate == name) {
        return char::from_u32(0xA0 + position as u32);
    }
    let code: u32 = match name {
        "amp" => 0x26,
        "lt" => 0x3C,
        "gt" => 0x3E,
        "quot" => 0x22,
        "apos" => 0x27,
        "ensp" => 0x2002,
        "emsp" => 0x2003,
        "thinsp" => 0x2009,
        "zwnj" => 0x200C,
        "zwj" => 0x200D,
        "lrm" => 0x200E,
        "rlm" => 0x200F,
        "ndash" => 0x2013,
        "mdash" => 0x2014,
        "lsquo" => 0x2018,
        "rsquo" => 0x2019,
        "sbquo" => 0x201A,
        "ldquo" => 0x201C,
        "rdquo" => 0x201D,
        "bdquo" => 0x201E,
        "dagger" => 0x2020,
        "Dagger" => 0x2021,
        "bull" => 0x2022,
        "hellip" => 0x2026,
        "permil" => 0x2030,
        "lsaquo" => 0x2039,
        "rsaquo" => 0x203A,
        "euro" => 0x20AC,
        "trade" => 0x2122,
        "larr" => 0x2190,
        "uarr" => 0x2191,
        "rarr" => 0x2192,
        "darr" => 0x2193,
        "minus" => 0x2212,
        "ne" => 0x2260,
        "le" => 0x2264,
        "ge" => 0x2265,
        "infin" => 0x221E,
        "hearts" => 0x2665,
        "OElig" => 0x0152,
        "oelig" => 0x0153,
        "Scaron" => 0x0160,
        "scaron" => 0x0161,
        "Yuml" => 0x0178,
        "fnof" => 0x0192,
        "circ" => 0x02C6,
        "tilde" => 0x02DC,
        _ => return None,
    };
    char::from_u32(code)
}

/// The entity starting at the `&` at `amp`, and the index just past its `;`. `None` when it is
/// not an entity (a bare `&`, no `;` within reach, or a malformed number), which HTML treats as
/// text.
fn parse_entity(chars: &[char], amp: usize) -> Option<(Entity, usize)> {
    let mut j = amp + 1;
    let mut body = String::new();
    while let Some(&c) = chars.get(j) {
        if c == ';' {
            break;
        }
        let allowed = c.is_ascii_alphanumeric() || (c == '#' && body.is_empty());
        if body.len() >= 32 || !allowed {
            return None;
        }
        body.push(c);
        j += 1;
    }
    if chars.get(j) != Some(&';') || body.is_empty() {
        return None;
    }
    let end = j + 1;
    if let Some(number) = body.strip_prefix('#') {
        let value = match number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse::<u32>().ok(),
        }?;
        return Some((Entity::Char(numeric_reference(value)), end));
    }
    match named_entity(&body) {
        Some(c) => Some((Entity::Char(c), end)),
        None => Some((Entity::Unresolved, end)),
    }
}

pub(crate) struct HtmlWords {
    pub(crate) words: Vec<String>,
    /// Named entities outside the built-in table (dropped, not guessed).
    pub(crate) unresolved_entities: u64,
}

/// The visible words of (preprocessed) HTML, as described in the module comment.
pub(crate) fn html_visible_words(html: &str) -> HtmlWords {
    let chars: Vec<char> = html.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut text = String::with_capacity(html.len() / 2);
    let mut unresolved = 0u64;
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '<' => {
                if chars.get(i + 1).is_some_and(|c| *c == '!' || *c == '?') {
                    // A doctype, declaration, or processing instruction.
                    text.push(' ');
                    i = tag_end(&chars, i);
                    continue;
                }
                let Some((name, closing)) = tag_name(&chars, i) else {
                    text.push('<');
                    i += 1;
                    continue;
                };
                let end = tag_end(&chars, i);
                if is_block_tag(&name) {
                    text.push(' ');
                }
                if name == "head"
                    && !closing
                    && let Some(close) = head_close_end(&chars, &lower, end)
                {
                    text.push(' ');
                    i = close;
                    continue;
                }
                i = end;
            }
            '&' => match parse_entity(&chars, i) {
                Some((Entity::Char(c), end)) => {
                    text.push(c);
                    i = end;
                }
                Some((Entity::Unresolved, end)) => {
                    unresolved += 1;
                    i = end;
                }
                None => {
                    text.push('&');
                    i += 1;
                }
            },
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    HtmlWords {
        words: to_words(&text),
        unresolved_entities: unresolved,
    }
}

// ----- Markdown side ---------------------------------------------------------

/// Nesting of links inside link text that is followed (an image inside a link is the usual
/// case); deeper text is kept as written.
const MAX_LINK_DEPTH: usize = 16;

struct LinkExtent {
    /// The index of the `]` that closes the text.
    text_end: usize,
    /// The index just past the `)` that closes the destination.
    end: usize,
}

/// The extent of `[text](destination)` whose `[` is at `open`, or `None` when it is not a link
/// (no matching `]`, no `(` right after it, or an unclosed destination).
fn link_extent(chars: &[char], open: usize) -> Option<LinkExtent> {
    let mut depth = 0usize;
    let mut i = open;
    let close = loop {
        match *chars.get(i)? {
            '\\' => {
                i += 2;
                continue;
            }
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break i;
                }
            }
            _ => {}
        }
        i += 1;
    };
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut j = close + 2;
    if chars.get(j) == Some(&'<') {
        while let Some(&c) = chars.get(j) {
            j += 1;
            if c == '>' {
                break;
            }
        }
    }
    let mut parentheses = 1usize;
    while let Some(&c) = chars.get(j) {
        match c {
            '\\' => {
                j += 2;
                continue;
            }
            '(' => parentheses += 1,
            ')' => {
                parentheses -= 1;
                if parentheses == 0 {
                    return Some(LinkExtent {
                        text_end: close,
                        end: j + 1,
                    });
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn markdown_text(chars: &[char], depth: usize) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => match chars.get(i + 1).copied() {
                Some(next) if next.is_ascii_punctuation() => {
                    out.push(next);
                    i += 2;
                }
                Some('\n') => {
                    out.push(' ');
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            },
            '!' if chars.get(i + 1) == Some(&'[') => match link_extent(chars, i + 1) {
                // An image: its alt text and destination are not words.
                Some(extent) => i = extent.end,
                None => {
                    out.push('!');
                    i += 1;
                }
            },
            '[' => match link_extent(chars, i) {
                Some(extent) if depth < MAX_LINK_DEPTH => {
                    out.push_str(&markdown_text(&chars[i + 1..extent.text_end], depth + 1));
                    i = extent.end;
                }
                _ => {
                    out.push('[');
                    i += 1;
                }
            },
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The words of converted Markdown, as described in the module comment.
pub(crate) fn markdown_words(markdown: &str) -> Vec<String> {
    let chars: Vec<char> = markdown.chars().collect();
    to_words(&markdown_text(&chars, 0))
        .into_iter()
        .map(|word| {
            let autolink = word.len() > 2
                && word.starts_with('<')
                && word.ends_with('>')
                && (word.contains("://") || word.starts_with("<mailto:"));
            if autolink {
                word[1..word.len() - 1].to_string()
            } else {
                word
            }
        })
        .collect()
}

// =============================================================================
// Comparison
// =============================================================================

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WordComparison {
    /// Words equal on both sides, in order.
    pub(crate) matched: u64,
    /// HTML words the Markdown does not have at that place.
    pub(crate) missing: u64,
    /// Markdown words (not syntax words) the HTML does not have at that place.
    pub(crate) extra: u64,
    /// Markdown syntax words excused (see the module comment).
    pub(crate) excused: u64,
    pub(crate) missing_runs: u64,
    pub(crate) extra_runs: u64,
    /// Places where neither side had the other's word within the window (one word on each side
    /// is counted as missing and extra).
    pub(crate) unresynced: u64,
}

impl WordComparison {
    pub(crate) fn differences(&self) -> u64 {
        self.missing + self.extra
    }

    fn add(&mut self, other: &WordComparison) {
        self.matched += other.matched;
        self.missing += other.missing;
        self.extra += other.extra;
        self.excused += other.excused;
        self.missing_runs += other.missing_runs;
        self.extra_runs += other.extra_runs;
        self.unresynced += other.unresynced;
    }
}

/// Counts the Markdown words in `words`, splitting them into excused syntax words and extras;
/// returns the number of extras.
fn tally_markdown_run(words: &[String], result: &mut WordComparison) -> u64 {
    let mut extras = 0u64;
    for word in words {
        if is_markdown_syntax_word(word) {
            result.excused += 1;
        } else {
            extras += 1;
        }
    }
    result.extra += extras;
    if extras > 0 {
        result.extra_runs += 1;
    }
    extras
}

/// Compares the HTML's words with the Markdown's, in order (see the module comment).
/// `html.len() == matched + missing` and `markdown.len() == matched + extra + excused` always.
pub(crate) fn compare_words(html: &[String], markdown: &[String]) -> WordComparison {
    let mut result = WordComparison::default();
    let (mut i, mut j) = (0usize, 0usize);
    while i < html.len() && j < markdown.len() {
        if html[i] == markdown[j] {
            result.matched += 1;
            i += 1;
            j += 1;
            continue;
        }
        if is_markdown_syntax_word(&markdown[j]) {
            result.excused += 1;
            j += 1;
            continue;
        }
        let skip_html =
            (1..=RESYNC_WINDOW).find(|k| html.get(i + k).is_some_and(|w| *w == markdown[j]));
        let skip_markdown =
            (1..=RESYNC_WINDOW).find(|k| markdown.get(j + k).is_some_and(|w| *w == html[i]));
        match (skip_html, skip_markdown) {
            (Some(h), Some(m)) if h <= m => {
                result.missing += h as u64;
                result.missing_runs += 1;
                i += h;
            }
            (Some(h), None) => {
                result.missing += h as u64;
                result.missing_runs += 1;
                i += h;
            }
            (_, Some(m)) => {
                tally_markdown_run(&markdown[j..j + m], &mut result);
                j += m;
            }
            (None, None) => {
                result.unresynced += 1;
                result.missing += 1;
                result.missing_runs += 1;
                result.extra += 1;
                result.extra_runs += 1;
                i += 1;
                j += 1;
            }
        }
    }
    if i < html.len() {
        result.missing += (html.len() - i) as u64;
        result.missing_runs += 1;
    }
    if j < markdown.len() {
        tally_markdown_run(&markdown[j..], &mut result);
    }
    result
}

/// What checking one message's HTML against its Markdown gave.
pub(crate) struct CheckedBody {
    pub(crate) html_words: Vec<String>,
    pub(crate) markdown_words: Vec<String>,
    pub(crate) unresolved_entities: u64,
    pub(crate) comparison: WordComparison,
}

/// Preprocesses `html` as the export does, takes its visible words, and compares them with the
/// words of `markdown` (the Markdown the export wrote for it).
pub(crate) fn check_html_against_markdown(html: &str, markdown: &str) -> CheckedBody {
    let (preprocessed, _applied) = preprocess_html_detailed(html);
    let html_words = html_visible_words(&preprocessed);
    let markdown_words = markdown_words(markdown);
    let comparison = compare_words(&html_words.words, &markdown_words);
    CheckedBody {
        html_words: html_words.words,
        markdown_words,
        unresolved_entities: html_words.unresolved_entities,
        comparison,
    }
}

// =============================================================================
// --dump-bodies
// =============================================================================

/// The directory `--dump-bodies` writes into, and how many files it has written. Error messages
/// never include the path, and no file name comes from the message.
pub(crate) struct WordsDump {
    path: PathBuf,
    files_written: u64,
}

impl WordsDump {
    /// A new directory is created; an existing one must be empty, so a dump never mixes with, or
    /// replaces, anything else.
    pub(crate) fn prepare(path: &Path) -> Result<WordsDump> {
        if path.exists() {
            if !path.is_dir() {
                anyhow::bail!("the --dump-bodies path exists and is not a directory");
            }
            let mut entries =
                fs::read_dir(path).context("failed to read the --dump-bodies directory")?;
            if entries.next().is_some() {
                anyhow::bail!(
                    "the --dump-bodies directory is not empty; name a new or empty directory"
                );
            }
        } else {
            fs::create_dir_all(path).context("failed to create the --dump-bodies directory")?;
        }
        Ok(WordsDump {
            path: path.to_path_buf(),
            files_written: 0,
        })
    }

    /// Writes one word per line to `NNN.<label>.words.txt`, `NNN` being the message's 1-based
    /// position in the scanned list.
    fn write_words(&mut self, index: usize, label: &str, words: &[String]) -> Result<()> {
        let mut text = words.join("\n");
        text.push('\n');
        fs::write(
            self.path.join(format!("{index:03}.{label}.words.txt")),
            text,
        )
        .context("failed to write a --dump-bodies file")?;
        self.files_written += 1;
        Ok(())
    }
}

// =============================================================================
// The corpus mode
// =============================================================================

#[derive(Default)]
pub(crate) struct BodiesTotals {
    open_errors: u64,
    messages_read: u64,
    selected_native_html: u64,
    selected_rtf_html: u64,
    selected_plain_text: u64,
    selected_none: u64,
    html_bodies_checked: u64,
    /// An HTML body was selected but the converter failed (the export falls back to plain text).
    html_conversions_failed: u64,
    words_html: u64,
    words_markdown: u64,
    comparison: WordComparison,
    unresolved_entities: u64,
    messages_identical: u64,
    /// The messages with a difference: position in the scanned list and their comparison.
    differing: Vec<(usize, WordComparison)>,
}

/// Scans `files`, checking each message whose selected body is HTML. With `dump` set, writes the
/// two word lists of every message with a difference. A failure to write is an error; everything
/// else is counted.
pub(crate) fn collect_bodies_verify_totals(
    files: &[PathBuf],
    dump: &mut Option<WordsDump>,
) -> Result<BodiesTotals> {
    let mut totals = BodiesTotals::default();
    for (position, file) in files.iter().enumerate() {
        let index = position + 1;
        let Ok(content) = read_message_content(file) else {
            totals.open_errors += 1;
            continue;
        };
        totals.messages_read += 1;
        let selection = select_body(&content.bodies);
        match selection.source {
            BodySource::NativeHtml => totals.selected_native_html += 1,
            BodySource::RtfHtml => totals.selected_rtf_html += 1,
            BodySource::PlainText => totals.selected_plain_text += 1,
            BodySource::None => totals.selected_none += 1,
        }
        let Some(html) = selection.html.as_deref() else {
            continue;
        };
        let outcome = body_outcome(&content.bodies);
        let Some(markdown) = outcome.rendered.markdown.as_deref() else {
            totals.html_conversions_failed += 1;
            continue;
        };
        let checked = check_html_against_markdown(html, markdown);
        totals.html_bodies_checked += 1;
        totals.words_html += checked.html_words.len() as u64;
        totals.words_markdown += checked.markdown_words.len() as u64;
        totals.unresolved_entities += checked.unresolved_entities;
        totals.comparison.add(&checked.comparison);
        if checked.comparison.differences() == 0 {
            totals.messages_identical += 1;
        } else {
            totals.differing.push((index, checked.comparison.clone()));
            if let Some(dump) = dump.as_mut() {
                dump.write_words(index, "html", &checked.html_words)?;
                dump.write_words(index, "markdown", &checked.markdown_words)?;
            }
        }
    }
    Ok(totals)
}

pub(crate) fn print_bodies_verify_report(totals: &BodiesTotals) {
    let c = &totals.comparison;
    println!("open_errors={}", totals.open_errors);
    println!("messages_read={}", totals.messages_read);
    println!("body_selected_native_html={}", totals.selected_native_html);
    println!("body_selected_rtf_html={}", totals.selected_rtf_html);
    println!("body_selected_plain_text={}", totals.selected_plain_text);
    println!("body_selected_none={}", totals.selected_none);
    println!("html_bodies_checked={}", totals.html_bodies_checked);
    println!("html_conversions_failed={}", totals.html_conversions_failed);
    println!("words_html_total={}", totals.words_html);
    println!("words_markdown_total={}", totals.words_markdown);
    println!("words_matched_in_order={}", c.matched);
    println!("words_missing_from_markdown={}", c.missing);
    println!("words_extra_in_markdown={}", c.extra);
    println!("markdown_syntax_words_excused={}", c.excused);
    println!("missing_runs={}", c.missing_runs);
    println!("extra_runs={}", c.extra_runs);
    println!("unresynced_words={}", c.unresynced);
    println!(
        "html_unresolved_entities_total={}",
        totals.unresolved_entities
    );
    println!("messages_identical={}", totals.messages_identical);
    println!("messages_with_difference={}", totals.differing.len());
    for (index, comparison) in &totals.differing {
        println!(
            "difference_message_{index:03}_missing={}",
            comparison.missing
        );
        println!("difference_message_{index:03}_extra={}", comparison.extra);
        println!(
            "difference_message_{index:03}_missing_runs={}",
            comparison.missing_runs
        );
        println!(
            "difference_message_{index:03}_extra_runs={}",
            comparison.extra_runs
        );
        println!(
            "difference_message_{index:03}_unresynced={}",
            comparison.unresynced
        );
    }
    println!(
        "body_check_gate_violations={}",
        totals.differing.len() as u64 + totals.html_conversions_failed + totals.open_errors
    );
}

pub(crate) fn run_bodies_verify(
    files: &[PathBuf],
    subdirectories_skipped: u64,
    dump_dir: Option<&Path>,
) -> Result<()> {
    // Checked before any scanning, so a bad directory fails fast.
    let mut dump = dump_dir.map(WordsDump::prepare).transpose()?;
    let totals = collect_bodies_verify_totals(files, &mut dump)?;
    println!("inventory=privacy_safe");
    println!("input_kind=msg_bodies_verify");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");
    print_bodies_verify_report(&totals);
    if let Some(dump) = &dump {
        println!("dump_files_written={}", dump.files_written);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BodyForms;
    use proptest::prelude::*;
    use std::io::Write;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn temp_path(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tsp-vb-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let _ = fs::remove_file(&path);
        path
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn check_native_html(html: &str) -> CheckedBody {
        let forms = BodyForms {
            native_html: Some(html.as_bytes().to_vec()),
            ..BodyForms::default()
        };
        let selection = select_body(&forms);
        let outcome = body_outcome(&forms);
        let markdown = outcome.rendered.markdown.expect("converted");
        check_html_against_markdown(selection.html.as_deref().expect("html"), &markdown)
    }

    // ----- HTML words -----

    #[test]
    fn inline_elements_join_words_and_block_elements_separate_them() {
        let html = "<p>Hello <b>big</b>wor<span>ld</span></p><p>x</p>a<br>b";
        assert_eq!(
            html_visible_words(html).words,
            words(&["Hello", "bigworld", "x", "a", "b"])
        );
        assert_eq!(
            html_visible_words("<td>a</td><td>b</td>").words,
            words(&["a", "b"])
        );
    }

    #[test]
    fn entities_are_decoded_and_unknown_names_are_counted() {
        let html = "a&nbsp;b &amp; c &#185; &#150; &#x41; &bogus; &eacute; & x &#;";
        let result = html_visible_words(html);
        assert_eq!(
            result.words,
            words(&[
                "a", "b", "&", "c", "\u{b9}", "\u{2013}", "A", "\u{e9}", "&", "x", "&#;"
            ])
        );
        assert_eq!(result.unresolved_entities, 1);
    }

    #[test]
    fn tags_and_attributes_are_never_words() {
        let html = "<img alt=\"x y\" src=\"a>b\"><a href='q r' title=it's>link</a>";
        assert_eq!(html_visible_words(html).words, words(&["link"]));
        // A `<` that cannot start a tag is text.
        assert_eq!(html_visible_words("a < b").words, words(&["a", "<", "b"]));
        // A doctype is markup.
        assert_eq!(
            html_visible_words("<!DOCTYPE html><p>x</p>").words,
            words(&["x"])
        );
    }

    #[test]
    fn a_closed_head_is_excluded_and_an_unclosed_one_is_kept() {
        let closed = "<html><head><meta charset=x><noscript>hidden</noscript></head><body>shown</body></html>";
        assert_eq!(html_visible_words(closed).words, words(&["shown"]));
        assert_eq!(html_visible_words("<head>text").words, words(&["text"]));
        assert_eq!(html_visible_words("<HEAD>a</HEAD>b").words, words(&["b"]));
    }

    #[test]
    fn emphasis_marks_and_padding_characters_are_removed_from_words() {
        assert_eq!(
            html_visible_words("snake_case **x** a\u{200B}b \u{034F}\u{00AD} end").words,
            words(&["snakecase", "x", "ab", "end"])
        );
    }

    // ----- Markdown words -----

    #[test]
    fn markdown_links_images_and_escapes_reduce_to_their_words() {
        let md = "# Title\n\n**Bold** and [text](http://x/a_(b)) ![alt text](cid:x) \\*star\\* [![i](s)](h)\n- item\n1. one";
        assert_eq!(
            markdown_words(md),
            words(&[
                "#", "Title", "Bold", "and", "text", "star", "-", "item", "1.", "one"
            ])
        );
    }

    #[test]
    fn link_text_may_hold_brackets_and_links_may_use_angle_destinations() {
        assert_eq!(
            markdown_words("[a \\[b\\] c](<http://x/y z>) d"),
            words(&["a", "[b]", "c", "d"])
        );
        // Text that only looks like a link opener is kept.
        assert_eq!(markdown_words("a [b c"), words(&["a", "[b", "c"]));
        assert_eq!(markdown_words("a ![b c"), words(&["a", "![b", "c"]));
        // An unwrapped autolink keeps its text.
        assert_eq!(
            markdown_words("see <http://x.example/y> now"),
            words(&["see", "http://x.example/y", "now"])
        );
    }

    #[test]
    fn syntax_words_are_recognized_narrowly() {
        for syntax in [
            "#", "###", ">", ">>", "-", "---", "+", "|", ":--", "===", "1.", "12)",
        ] {
            assert!(is_markdown_syntax_word(syntax), "{syntax}");
        }
        for text in [
            "",
            "-a",
            "a-",
            "1",
            "a.",
            ".",
            "1.5",
            "(1)",
            "1.)",
            "1234567890.",
        ] {
            assert!(!is_markdown_syntax_word(text), "{text}");
        }
    }

    // ----- comparison -----

    #[test]
    fn identical_words_match_and_markdown_syntax_is_excused() {
        let html = words(&["Title", "Bold", "and", "text", "star", "item", "one"]);
        let md = words(&[
            "#", "Title", "Bold", "and", "text", "star", "-", "item", "1.", "one",
        ]);
        let result = compare_words(&html, &md);
        assert_eq!((result.matched, result.excused), (7, 3));
        assert_eq!(result.differences(), 0);
    }

    #[test]
    fn a_syntax_word_that_the_html_really_has_matches_instead_of_being_excused() {
        let html = words(&["a", "-", "b"]);
        let result = compare_words(&html, &words(&["a", "-", "b"]));
        assert_eq!((result.matched, result.excused), (3, 0));
        // Lost from the Markdown: the HTML's literal dash is reported.
        let lost = compare_words(&html, &words(&["a", "b"]));
        assert_eq!((lost.matched, lost.missing), (2, 1));
    }

    #[test]
    fn a_lost_word_is_a_missing_run() {
        let result = compare_words(
            &words(&["a", "b", "c", "d", "e"]),
            &words(&["a", "b", "d", "e"]),
        );
        assert_eq!(
            (
                result.matched,
                result.missing,
                result.missing_runs,
                result.extra
            ),
            (4, 1, 1, 0)
        );
    }

    #[test]
    fn an_added_word_is_an_extra_run() {
        let result = compare_words(&words(&["a", "b", "c"]), &words(&["a", "X", "b", "c"]));
        assert_eq!(
            (
                result.matched,
                result.extra,
                result.extra_runs,
                result.missing
            ),
            (3, 1, 1, 0)
        );
    }

    #[test]
    fn tails_are_counted_and_unrelated_words_do_not_resynchronize() {
        let tail = compare_words(&words(&["a", "b", "c"]), &words(&["a"]));
        assert_eq!((tail.matched, tail.missing, tail.missing_runs), (1, 2, 1));
        let extra_tail = compare_words(&words(&["a"]), &words(&["a", "x", "-", "y"]));
        assert_eq!((extra_tail.extra, extra_tail.excused), (2, 1));
        let apart = compare_words(&words(&["a"]), &words(&["b"]));
        assert_eq!((apart.unresynced, apart.missing, apart.extra), (1, 1, 1));
        assert_eq!(compare_words(&[], &[]), WordComparison::default());
    }

    proptest! {
        #[test]
        fn the_counts_always_reconcile(
            html in prop::collection::vec("[a-d]{1,2}", 0..40),
            markdown in prop::collection::vec("[a-d#>-]{1,2}", 0..40),
        ) {
            let result = compare_words(&html, &markdown);
            prop_assert_eq!(html.len() as u64, result.matched + result.missing);
            prop_assert_eq!(
                markdown.len() as u64,
                result.matched + result.extra + result.excused
            );
        }

        #[test]
        fn equal_lists_never_differ(items in prop::collection::vec("[a-d]{1,3}", 0..40)) {
            prop_assert_eq!(compare_words(&items, &items).differences(), 0);
        }
    }

    // ----- the real converter -----

    #[test]
    fn a_typical_body_converts_without_losing_a_word_or_order() {
        let html = "<html><head><title>Title</title></head><body><h1>Heading</h1>\
            <p>Hello <b>big</b> <a href=\"http://example.com/page\">link text</a> and <i>italic</i>.</p>\
            <ul><li>one</li><li>two</li></ul><ol><li>first</li><li>second</li></ol>\
            <p><img src=\"cid:x@y\" alt=\"a picture\"> tail&nbsp;text &amp; more</p></body></html>";
        let checked = check_native_html(html);
        assert_eq!(
            checked.html_words,
            words(&[
                "Heading", "Hello", "big", "link", "text", "and", "italic.", "one", "two", "first",
                "second", "tail", "text", "&", "more"
            ])
        );
        assert_eq!(
            checked.comparison.differences(),
            0,
            "markdown words: {:?}",
            checked.markdown_words
        );
        assert_eq!(checked.comparison.matched, 15);
    }

    // ----- the corpus mode -----

    #[test]
    fn a_synthetic_message_with_html_is_checked_and_one_without_is_only_counted() {
        let dir = temp_path("collect");
        fs::create_dir_all(&dir).expect("create temp dir");

        let html_msg = dir.join("a.msg");
        {
            let mut comp = cfb::create(&html_msg).expect("create CFB");
            put(&mut comp, "/__properties_version1.0", &[0u8; 32]);
            put(
                &mut comp,
                "/__substg1.0_10130102",
                b"<p>Hello <b>big</b> world</p><ul><li>one</li></ul>",
            );
        }
        let plain_msg = dir.join("b.msg");
        {
            let mut comp = cfb::create(&plain_msg).expect("create CFB");
            put(&mut comp, "/__properties_version1.0", &[0u8; 32]);
            put(&mut comp, "/__substg1.0_1000001F", &utf16("plain"));
        }
        let broken = dir.join("c.msg");
        fs::write(&broken, b"not a compound file").expect("write broken");

        let totals = collect_bodies_verify_totals(&[html_msg, plain_msg, broken], &mut None)
            .expect("collect");
        assert_eq!(totals.messages_read, 2);
        assert_eq!(totals.open_errors, 1);
        assert_eq!(totals.selected_native_html, 1);
        assert_eq!(totals.selected_plain_text, 1);
        assert_eq!(totals.html_bodies_checked, 1);
        assert_eq!(totals.html_conversions_failed, 0);
        assert_eq!(totals.comparison.differences(), 0);
        assert_eq!(totals.messages_identical, 1);
        assert_eq!(totals.words_html, 4);
        let _ = fs::remove_dir_all(&dir);
    }

    // ----- --dump-bodies -----

    #[test]
    fn a_new_directory_is_created_and_word_lists_are_written_one_per_line() {
        let path = temp_path("dump-new");
        let mut dump = WordsDump::prepare(&path).expect("prepare");
        dump.write_words(7, "html", &words(&["a", "b"]))
            .expect("write");
        assert_eq!(dump.files_written, 1);
        assert_eq!(
            fs::read_to_string(path.join("007.html.words.txt")).expect("read back"),
            "a\nb\n"
        );
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn a_busy_directory_or_a_file_is_refused_and_errors_do_not_name_the_path() {
        let busy = temp_path("dump-busy");
        fs::create_dir_all(&busy).expect("create");
        fs::write(busy.join("keep.txt"), b"mine").expect("write");
        assert!(WordsDump::prepare(&busy).is_err());
        assert_eq!(fs::read(busy.join("keep.txt")).expect("read back"), b"mine");
        let _ = fs::remove_dir_all(&busy);

        let file = temp_path("dump-secret-name-segment");
        fs::write(&file, b"a file").expect("write");
        let error = WordsDump::prepare(&file).err().expect("refused");
        assert!(!format!("{error:#}").contains("secret-name-segment"));
        let _ = fs::remove_file(&file);
    }
}
