//! MS-OXRTFEX de-encapsulation (M4e-1): recover the original HTML from the decompressed RTF of a
//! message whose `PidTagRtfCompressed` encapsulates HTML (the `\fromhtml1` form).
//!
//! Pure: bytes in, text and diagnostics out. No I/O, no adapter types, no panics on any input
//! (bounded group depth and bounded output; damage is counted in [`Diagnostics`], never raised).
//!
//! What it implements, from MS-OXRTFEX 2.2.3.1 (recognition) and 2.2.3.2 (extraction):
//!
//! - **Recognition.** The document must start with `{\rtf1`; at most the first 10 tokens (begin
//!   group marks and control words) are inspected; `\fromhtml1` among them means encapsulated
//!   HTML, `\fromtext` encapsulated plain text; any other kind of token, or neither word, means
//!   ordinary RTF. Note this is stricter than the whole-document search for `\fromhtml1` that
//!   [`crate::shared::rtf_bytes_contain_fromhtml`] performs; `--verify-deencap` compares the two.
//! - **`\*\htmltag` groups.** Their content is copied to the HTML. The numeric parameter after
//!   `htmltag` is ignored. Inside them `\'hh` and `\uN` are decoded (bytes in the document's
//!   default code page), `\par` becomes CRLF and `\tab` a tab, and other control words are
//!   ignored.
//! - **`\htmlrtf` / `\htmlrtf0`.** Outside `htmltag` groups, text and control words are
//!   suppressed while the toggle is on, except that the current font is still tracked. The toggle
//!   is scoped to its group like any RTF character-formatting toggle (an assumption, checked
//!   against the corpus by `--verify-deencap`).
//! - **Everything else outside `htmltag`.** Ignorable destinations (`{\*\...}`) other than
//!   `htmltag` are skipped (this includes `\*\mhtmltag`); standard destinations that produce no
//!   visible text are skipped (a fixed list, below); the font table is read for each font's code
//!   page; remaining text is copied, with `\'hh` decoded in the current font's code page,
//!   `\uN` honouring `\ucN`, `\par` and `\line` as CRLF, `\tab` as a tab, and the RTF control
//!   words and symbols that stand for a Unicode character converted.
//! - **Output encoding.** The result is a Rust `String` (UTF-8). A `<meta ... charset=...>` in the
//!   recovered HTML therefore describes the original bytes, not this text; the diagnostics say
//!   whether one was seen so the body pipeline can decide. Code pages the project's decoders do
//!   not implement (see `oxmsg_decode`) yield U+FFFD per byte and are counted, never guessed.
//!
//! Known limits, reported rather than hidden: standard destinations not in the fixed list are
//! indistinguishable from formatting groups and are treated as visible; double-byte code pages
//! are unsupported (their bytes become U+FFFD); `\objattph` placeholders are not mapped.

use std::collections::{BTreeMap, BTreeSet};

use crate::oxmsg_decode::{String8Decoded, decode_string8_with_codepage};

/// Default bound on group nesting; deeper input stops the scan and sets
/// [`Diagnostics::depth_limit_hit`].
pub(crate) const DEFAULT_MAX_DEPTH: usize = 1024;
/// Default bound on the recovered HTML, in bytes; more output stops the scan and sets
/// [`Diagnostics::output_limit_hit`].
pub(crate) const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Standard RTF destinations that produce no visible text (MS-OXRTFEX: "ignore and skip any
/// standard RTF destination groups that do not produce visible text ... except `\fonttbl`").
/// Only recognized as the first word of a group.
const STANDARD_SKIPPED_DESTINATIONS: &[&str] = &[
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "footnote",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "themedata",
    "colormapping",
    "datastore",
    "latentstyles",
    "xmlnstbl",
    "pgptbl",
    "revtbl",
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) max_depth: usize,
    pub(crate) max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: DEFAULT_MAX_DEPTH,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// What the RTF document is, by the MS-OXRTFEX recognition rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RtfKind {
    EncapsulatedHtml,
    EncapsulatedText,
    PlainRtf,
    NotRtf,
}

impl RtfKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RtfKind::EncapsulatedHtml => "encapsulated_html",
            RtfKind::EncapsulatedText => "encapsulated_text",
            RtfKind::PlainRtf => "plain_rtf",
            RtfKind::NotRtf => "not_rtf",
        }
    }
}

/// Counts of what the scan met. Content-free: counts, flags, and code page numbers only.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostics {
    /// The default RTF code page used for `htmltag` content (`\ansicpgN`, else 1252).
    pub(crate) default_codepage: u32,
    pub(crate) default_codepage_declared: bool,
    pub(crate) htmltag_groups: u64,
    pub(crate) ignorable_destinations_skipped: u64,
    pub(crate) standard_destinations_skipped: u64,
    /// Times `\htmlrtf` turned suppression on (from off).
    pub(crate) htmlrtf_regions: u64,
    /// `\'hh` escapes decoded into output.
    pub(crate) hex_escapes: u64,
    pub(crate) bad_hex_escapes: u64,
    pub(crate) unicode_escapes: u64,
    pub(crate) unpaired_surrogates: u64,
    pub(crate) paragraph_breaks: u64,
    pub(crate) tabs: u64,
    /// Control symbols met in emitting context that have no text equivalent.
    pub(crate) control_symbols_ignored: u64,
    pub(crate) binary_bytes_skipped: u64,
    /// `\object` groups skipped (embedded objects have no HTML equivalent).
    pub(crate) object_placeholders: u64,
    /// Bytes that could not be decoded (unsupported code page), emitted as U+FFFD.
    pub(crate) undecodable_bytes: u64,
    pub(crate) unsupported_codepages: BTreeSet<u32>,
    pub(crate) fonts_defined: u64,
    pub(crate) unclosed_groups_at_end: u64,
    pub(crate) stray_group_ends: u64,
    pub(crate) depth_limit_hit: bool,
    pub(crate) output_limit_hit: bool,
    /// The recovered HTML contains `charset=` (a stale declaration, since the text is UTF-8).
    pub(crate) meta_charset_declared: bool,
}

pub(crate) struct Deencapsulated {
    pub(crate) html: String,
    pub(crate) diagnostics: Diagnostics,
}

// =============================================================================
// Tokenizer
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Token<'a> {
    GroupStart,
    GroupEnd,
    Word {
        name: &'a [u8],
        param: Option<i32>,
    },
    /// A control symbol other than `\'hh` (`\\`, `\{`, `\}`, `\*`, `\~`, ...).
    Symbol(u8),
    /// `\'hh`.
    Hex(u8),
    /// `\'` not followed by two hex digits.
    BadHex,
    /// A backslash at the very end of the input: malformed, produces nothing.
    TrailingBackslash,
    Text(u8),
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Reads the control word or symbol that starts at `pos` (where `data[pos]` is a backslash).
/// Always advances by at least one byte.
fn read_control(data: &[u8], pos: usize) -> (Token<'_>, usize) {
    let Some(&next) = data.get(pos + 1) else {
        return (Token::TrailingBackslash, pos + 1);
    };

    if next.is_ascii_alphabetic() {
        let name_start = pos + 1;
        let mut name_end = name_start;
        while name_end < data.len() && data[name_end].is_ascii_alphabetic() {
            name_end += 1;
        }
        let name = &data[name_start..name_end];

        let mut end = name_end;
        let negative = data.get(end) == Some(&b'-');
        let digits_start = if negative { end + 1 } else { end };
        let mut digits_end = digits_start;
        while digits_end < data.len() && data[digits_end].is_ascii_digit() {
            digits_end += 1;
        }
        let mut param = None;
        if digits_end > digits_start {
            let mut value: i64 = 0;
            for &digit in &data[digits_start..digits_end] {
                value = (value * 10 + i64::from(digit - b'0')).min(i64::from(i32::MAX));
            }
            if negative {
                value = -value;
            }
            param = Some(value as i32);
            end = digits_end;
        }
        // One space after a control word is its delimiter and is not part of the text.
        if data.get(end) == Some(&b' ') {
            end += 1;
        }
        return (Token::Word { name, param }, end);
    }

    match next {
        b'\'' => {
            let high = data.get(pos + 2).copied().and_then(hex_value);
            let low = data.get(pos + 3).copied().and_then(hex_value);
            match (high, low) {
                (Some(high), Some(low)) => (Token::Hex(high * 16 + low), pos + 4),
                _ => (Token::BadHex, pos + 2),
            }
        }
        // A backslash before a line break is a paragraph break.
        b'\r' | b'\n' => (
            Token::Word {
                name: &b"par"[..],
                param: None,
            },
            pos + 2,
        ),
        other => (Token::Symbol(other), pos + 2),
    }
}

/// The token at `pos` and the position after it, or `None` at the end of the input.
fn next_token(data: &[u8], pos: usize) -> Option<(Token<'_>, usize)> {
    let byte = *data.get(pos)?;
    match byte {
        b'{' => Some((Token::GroupStart, pos + 1)),
        b'}' => Some((Token::GroupEnd, pos + 1)),
        b'\\' => Some(read_control(data, pos)),
        _ => Some((Token::Text(byte), pos + 1)),
    }
}

// =============================================================================
// Recognition (MS-OXRTFEX 2.2.3.1)
// =============================================================================

/// Classifies the document by the MS-OXRTFEX recognition rule: `{\rtf1` first, then at most the
/// first 10 tokens.
pub(crate) fn recognize(rtf: &[u8]) -> RtfKind {
    if !rtf.starts_with(b"{\\rtf1") {
        return RtfKind::NotRtf;
    }
    let mut pos = 0usize;
    let mut tokens_seen = 0usize;
    while tokens_seen < 10 {
        let Some((token, next)) = next_token(rtf, pos) else {
            break;
        };
        pos = next;
        match token {
            // Source line breaks are not tokens.
            Token::Text(b'\r') | Token::Text(b'\n') => {}
            Token::GroupStart => tokens_seen += 1,
            Token::Word { name, param } => {
                tokens_seen += 1;
                if name == b"fromhtml" && param == Some(1) {
                    return RtfKind::EncapsulatedHtml;
                }
                if name == b"fromtext" {
                    return RtfKind::EncapsulatedText;
                }
            }
            // Any other kind of token in the first 10 means ordinary RTF.
            _ => return RtfKind::PlainRtf,
        }
    }
    RtfKind::PlainRtf
}

// =============================================================================
// Code pages
// =============================================================================

/// The Windows code page for an RTF `\fcharsetN` value, where one is implied. `None` for the
/// default (1), symbol (2), and unknown charsets, which use the document's default code page.
fn codepage_for_charset(charset: i32) -> Option<u32> {
    match charset {
        0 => Some(1252),
        77 => Some(10000),
        128 => Some(932),
        129 => Some(949),
        130 => Some(1361),
        134 => Some(936),
        136 => Some(950),
        161 => Some(1253),
        162 => Some(1254),
        163 => Some(1258),
        177 => Some(1255),
        178 => Some(1256),
        186 => Some(1257),
        204 => Some(1251),
        222 => Some(874),
        238 => Some(1250),
        _ => None,
    }
}

/// The character an RTF control word stands for, where MS-OXRTFEX says such words are converted.
fn special_char(name: &[u8]) -> Option<char> {
    match name {
        b"lquote" => Some('\u{2018}'),
        b"rquote" => Some('\u{2019}'),
        b"ldblquote" => Some('\u{201C}'),
        b"rdblquote" => Some('\u{201D}'),
        b"bullet" => Some('\u{2022}'),
        b"endash" => Some('\u{2013}'),
        b"emdash" => Some('\u{2014}'),
        b"enspace" => Some('\u{2002}'),
        b"emspace" => Some('\u{2003}'),
        b"qmspace" => Some('\u{2005}'),
        b"zwj" => Some('\u{200D}'),
        b"zwnj" => Some('\u{200C}'),
        b"ltrmark" => Some('\u{200E}'),
        b"rtlmark" => Some('\u{200F}'),
        _ => None,
    }
}

// =============================================================================
// Output assembly
// =============================================================================

struct Emitter {
    out: String,
    /// Non-ASCII bytes waiting to be decoded together under `run_codepage`.
    run: Vec<u8>,
    run_codepage: u32,
    pending_high_surrogate: Option<u16>,
    limit: usize,
    limit_hit: bool,
    diag: Diagnostics,
}

impl Emitter {
    fn new(limit: usize) -> Self {
        Emitter {
            out: String::new(),
            run: Vec::new(),
            run_codepage: 1252,
            pending_high_surrogate: None,
            limit,
            limit_hit: false,
            diag: Diagnostics::default(),
        }
    }

    fn append(&mut self, text: &str) {
        if self.limit_hit {
            return;
        }
        if self.out.len() + text.len() > self.limit {
            self.limit_hit = true;
            self.diag.output_limit_hit = true;
            return;
        }
        self.out.push_str(text);
    }

    fn append_char(&mut self, c: char) {
        let mut buffer = [0u8; 4];
        let text: &str = c.encode_utf8(&mut buffer);
        self.append(text);
    }

    /// Decodes the waiting bytes under their code page and appends the text.
    fn flush_run(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.run);
        let codepage = if self.run_codepage == 0 {
            1252
        } else {
            self.run_codepage
        };
        match decode_string8_with_codepage(&bytes, codepage) {
            String8Decoded::Decoded { text, .. }
            | String8Decoded::AsciiUnderUnsupportedCodepage { text } => self.append(&text),
            String8Decoded::UnsupportedCodepage => {
                self.diag.undecodable_bytes += bytes.len() as u64;
                self.diag.unsupported_codepages.insert(codepage);
                for _ in 0..bytes.len() {
                    self.append("\u{FFFD}");
                }
            }
        }
    }

    fn flush_high_surrogate(&mut self) {
        if self.pending_high_surrogate.take().is_some() {
            self.diag.unpaired_surrogates += 1;
            self.append("\u{FFFD}");
        }
    }

    /// Writes out anything still waiting, in order.
    fn flush_pending(&mut self) {
        self.flush_run();
        self.flush_high_surrogate();
    }

    fn push_char(&mut self, c: char) {
        self.flush_pending();
        self.append_char(c);
    }

    fn push_str(&mut self, text: &str) {
        self.flush_pending();
        self.append(text);
    }

    /// One byte of text in `codepage`. ASCII is written directly; other bytes wait to be decoded
    /// as a run.
    fn push_byte(&mut self, byte: u8, codepage: u32) {
        if byte < 0x80 {
            self.push_char(char::from(byte));
            return;
        }
        self.flush_high_surrogate();
        if !self.run.is_empty() && self.run_codepage != codepage {
            self.flush_run();
        }
        self.run_codepage = codepage;
        self.run.push(byte);
    }

    /// One UTF-16 code unit from a `\uN` escape.
    fn push_utf16_unit(&mut self, unit: u16) {
        match (self.pending_high_surrogate.take(), unit) {
            (Some(high), 0xDC00..=0xDFFF) => {
                let scalar =
                    0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                self.push_char(char::from_u32(scalar).unwrap_or('\u{FFFD}'));
            }
            (Some(_), _) => {
                self.diag.unpaired_surrogates += 1;
                self.push_char('\u{FFFD}');
                self.push_utf16_unit(unit);
            }
            (None, 0xD800..=0xDBFF) => {
                self.flush_run();
                self.pending_high_surrogate = Some(unit);
            }
            (None, 0xDC00..=0xDFFF) => {
                self.diag.unpaired_surrogates += 1;
                self.push_char('\u{FFFD}');
            }
            (None, _) => {
                self.push_char(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
            }
        }
    }
}

// =============================================================================
// Interpreter
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dest {
    /// Ordinary document content.
    Normal,
    /// Inside `{\*\htmltag...}`: content is HTML.
    HtmlTag,
    /// Inside a skipped destination (and every group nested in it).
    Skip,
    /// Inside the font table.
    FontTable,
}

#[derive(Clone, Copy, Debug)]
struct GroupState {
    dest: Dest,
    htmlrtf: bool,
    /// `\ucN`: how many fallback characters follow each `\uN`.
    unicode_skip: u32,
    /// The current font number, -1 when none is selected.
    font: i32,
}

impl GroupState {
    fn initial() -> Self {
        GroupState {
            dest: Dest::Normal,
            htmlrtf: false,
            unicode_skip: 1,
            font: -1,
        }
    }
}

struct Interpreter<'a> {
    rtf: &'a [u8],
    pos: usize,
    limits: Limits,
    emitter: Emitter,
    fonts: BTreeMap<i32, u32>,
    default_codepage: u32,
    default_codepage_declared: bool,
    default_font: i32,
    font_being_defined: i32,
    stack: Vec<GroupState>,
    state: GroupState,
    /// The previous token was a group start, so a word now is the group's first word.
    after_group_start: bool,
    /// `{\*` was seen; the next word names the ignorable destination.
    star_pending: bool,
    /// Fallback characters still to skip after a `\uN`.
    fallback_to_skip: u32,
}

impl<'a> Interpreter<'a> {
    fn new(rtf: &'a [u8], limits: Limits) -> Self {
        Interpreter {
            rtf,
            pos: 0,
            limits,
            emitter: Emitter::new(limits.max_output_bytes),
            fonts: BTreeMap::new(),
            default_codepage: 1252,
            default_codepage_declared: false,
            default_font: -1,
            font_being_defined: -1,
            stack: Vec::new(),
            state: GroupState::initial(),
            after_group_start: false,
            star_pending: false,
            fallback_to_skip: 0,
        }
    }

    fn run(mut self) -> Deencapsulated {
        let rtf = self.rtf;
        while let Some((token, next)) = next_token(rtf, self.pos) {
            self.pos = next;
            if self.emitter.limit_hit {
                break;
            }
            // Source line breaks carry no meaning in RTF.
            if matches!(token, Token::Text(b'\r') | Token::Text(b'\n')) {
                continue;
            }
            // The fallback characters after a `\uN` are not text.
            if self.fallback_to_skip > 0 {
                if matches!(token, Token::Text(_) | Token::Hex(_)) {
                    self.fallback_to_skip -= 1;
                    continue;
                }
                self.fallback_to_skip = 0;
            }

            let was_group_start = self.after_group_start;
            self.after_group_start = false;

            match token {
                Token::GroupStart => {
                    if self.stack.len() >= self.limits.max_depth {
                        self.emitter.diag.depth_limit_hit = true;
                        break;
                    }
                    self.stack.push(self.state);
                    self.after_group_start = true;
                    self.star_pending = false;
                }
                Token::GroupEnd => {
                    self.emitter.flush_pending();
                    match self.stack.pop() {
                        Some(previous) => self.state = previous,
                        None => self.emitter.diag.stray_group_ends += 1,
                    }
                    self.star_pending = false;
                    self.fallback_to_skip = 0;
                }
                Token::Symbol(b'*') if was_group_start => {
                    self.star_pending = true;
                }
                Token::Symbol(symbol) => self.control_symbol(symbol),
                Token::Word { name, param } => self.word(name, param, was_group_start),
                Token::Hex(byte) => self.hex_escape(byte),
                Token::BadHex => self.emitter.diag.bad_hex_escapes += 1,
                Token::TrailingBackslash => {}
                Token::Text(byte) => self.text_byte(byte),
            }
        }
        self.finish()
    }

    fn finish(mut self) -> Deencapsulated {
        self.emitter.flush_pending();
        let mut diagnostics = self.emitter.diag;
        diagnostics.default_codepage = self.default_codepage;
        diagnostics.default_codepage_declared = self.default_codepage_declared;
        diagnostics.unclosed_groups_at_end = self.stack.len() as u64;
        let html = self.emitter.out;
        diagnostics.meta_charset_declared = html.to_ascii_lowercase().contains("charset=");
        Deencapsulated { html, diagnostics }
    }

    /// The code page for text in the current font (the default font when none is selected).
    fn current_codepage(&self) -> u32 {
        let font = if self.state.font >= 0 {
            self.state.font
        } else {
            self.default_font
        };
        self.fonts
            .get(&font)
            .copied()
            .unwrap_or(self.default_codepage)
    }

    /// Whether text and escapes at this point are written to the output, and in which code page.
    fn emitting_codepage(&self) -> Option<u32> {
        match self.state.dest {
            Dest::HtmlTag => Some(self.default_codepage),
            Dest::Normal if !self.state.htmlrtf => Some(self.current_codepage()),
            _ => None,
        }
    }

    fn text_byte(&mut self, byte: u8) {
        if let Some(codepage) = self.emitting_codepage() {
            self.emitter.push_byte(byte, codepage);
        }
    }

    fn hex_escape(&mut self, byte: u8) {
        if let Some(codepage) = self.emitting_codepage() {
            self.emitter.push_byte(byte, codepage);
            self.emitter.diag.hex_escapes += 1;
        }
    }

    fn control_symbol(&mut self, symbol: u8) {
        if self.emitting_codepage().is_none() {
            return;
        }
        match symbol {
            b'\\' | b'{' | b'}' => self.emitter.push_char(char::from(symbol)),
            b'~' => self.emitter.push_char('\u{00A0}'),
            b'_' => self.emitter.push_char('\u{2011}'),
            b'-' => self.emitter.push_char('\u{00AD}'),
            _ => self.emitter.diag.control_symbols_ignored += 1,
        }
    }

    fn classify_ignorable(&mut self, name: &[u8]) {
        match self.state.dest {
            Dest::Skip => {}
            Dest::HtmlTag => {
                self.state.dest = Dest::Skip;
                self.emitter.diag.ignorable_destinations_skipped += 1;
            }
            Dest::Normal | Dest::FontTable => {
                if name == b"htmltag" {
                    self.state.dest = Dest::HtmlTag;
                    self.emitter.diag.htmltag_groups += 1;
                } else {
                    self.state.dest = Dest::Skip;
                    self.emitter.diag.ignorable_destinations_skipped += 1;
                }
            }
        }
    }

    fn word(&mut self, name: &[u8], param: Option<i32>, was_group_start: bool) {
        if self.star_pending {
            self.star_pending = false;
            self.classify_ignorable(name);
            return;
        }
        if name == b"bin" {
            let wanted = usize::try_from(param.unwrap_or(0).max(0)).unwrap_or(0);
            let skipped = wanted.min(self.rtf.len().saturating_sub(self.pos));
            self.pos += skipped;
            self.emitter.diag.binary_bytes_skipped += skipped as u64;
            return;
        }
        match self.state.dest {
            Dest::Skip => {}
            Dest::FontTable => self.font_table_word(name, param),
            Dest::HtmlTag => self.emitting_word(name, param),
            Dest::Normal => self.normal_word(name, param, was_group_start),
        }
    }

    fn font_table_word(&mut self, name: &[u8], param: Option<i32>) {
        match name {
            b"f" => {
                self.font_being_defined = param.unwrap_or(-1);
                self.emitter.diag.fonts_defined += 1;
            }
            b"fcharset" => {
                if self.font_being_defined >= 0
                    && let Some(codepage) = param.and_then(codepage_for_charset)
                {
                    self.fonts
                        .entry(self.font_being_defined)
                        .or_insert(codepage);
                }
            }
            b"cpg" => {
                if self.font_being_defined >= 0
                    && let Some(codepage) = param.filter(|p| *p > 0)
                {
                    self.fonts.insert(self.font_being_defined, codepage as u32);
                }
            }
            _ => {}
        }
    }

    fn normal_word(&mut self, name: &[u8], param: Option<i32>, was_group_start: bool) {
        // Header and bookkeeping words act even where text is suppressed; the current font is
        // tracked inside `\htmlrtf` regions on purpose (MS-OXRTFEX 2.2.3.2).
        match name {
            b"ansicpg" => {
                if let Some(codepage) = param.filter(|p| *p > 0) {
                    self.default_codepage = codepage as u32;
                    self.default_codepage_declared = true;
                }
                return;
            }
            b"ansi" => {
                if !self.default_codepage_declared {
                    self.default_codepage = 1252;
                }
                return;
            }
            b"mac" => {
                if !self.default_codepage_declared {
                    self.default_codepage = 10000;
                }
                return;
            }
            b"pc" => {
                if !self.default_codepage_declared {
                    self.default_codepage = 437;
                }
                return;
            }
            b"pca" => {
                if !self.default_codepage_declared {
                    self.default_codepage = 850;
                }
                return;
            }
            b"deff" => {
                self.default_font = param.unwrap_or(-1);
                return;
            }
            b"f" => {
                self.state.font = param.unwrap_or(-1);
                return;
            }
            b"plain" => {
                self.state.font = -1;
                return;
            }
            b"uc" => {
                self.state.unicode_skip = u32::try_from(param.unwrap_or(1).max(0)).unwrap_or(1);
                return;
            }
            b"htmlrtf" => {
                let on = param.unwrap_or(1) != 0;
                if on && !self.state.htmlrtf {
                    self.emitter.diag.htmlrtf_regions += 1;
                }
                self.state.htmlrtf = on;
                return;
            }
            b"fonttbl" if was_group_start => {
                self.state.dest = Dest::FontTable;
                return;
            }
            _ => {}
        }
        if was_group_start
            && STANDARD_SKIPPED_DESTINATIONS
                .iter()
                .any(|skipped| name == skipped.as_bytes())
        {
            self.state.dest = Dest::Skip;
            self.emitter.diag.standard_destinations_skipped += 1;
            if name == b"object" {
                self.emitter.diag.object_placeholders += 1;
            }
            return;
        }
        if self.state.htmlrtf {
            return;
        }
        self.emitting_word(name, param);
    }

    /// A control word where text is being written (inside `htmltag`, or ordinary unsuppressed
    /// content). Words that do not stand for text are ignored.
    fn emitting_word(&mut self, name: &[u8], param: Option<i32>) {
        match name {
            b"par" | b"line" => {
                self.emitter.push_str("\r\n");
                self.emitter.diag.paragraph_breaks += 1;
            }
            b"tab" => {
                self.emitter.push_char('\t');
                self.emitter.diag.tabs += 1;
            }
            b"uc" => {
                // Also meaningful inside htmltag content.
                self.state.unicode_skip = u32::try_from(param.unwrap_or(1).max(0)).unwrap_or(1);
            }
            b"u" => {
                if let Some(value) = param {
                    // A signed 16-bit value: values above 32767 are written negative.
                    let unit = (value & 0xFFFF) as u16;
                    self.emitter.push_utf16_unit(unit);
                    self.emitter.diag.unicode_escapes += 1;
                    self.fallback_to_skip = self.state.unicode_skip;
                }
            }
            _ => {
                if let Some(c) = special_char(name) {
                    self.emitter.push_char(c);
                }
            }
        }
    }
}

/// Recovers the original HTML from decompressed RTF.
///
/// `Err` carries the document's [`RtfKind`] when it is not encapsulated HTML. Otherwise the
/// result is always `Ok`, however damaged the input: damage is in the diagnostics, and a stop at
/// a limit is flagged there (`depth_limit_hit`, `output_limit_hit`) with the HTML recovered so far.
pub(crate) fn deencapsulate_html(rtf: &[u8], limits: Limits) -> Result<Deencapsulated, RtfKind> {
    let kind = recognize(rtf);
    if kind != RtfKind::EncapsulatedHtml {
        return Err(kind);
    }
    Ok(Interpreter::new(rtf, limits).run())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn html(rtf: &[u8]) -> String {
        deencapsulate_html(rtf, Limits::default())
            .expect("encapsulated html")
            .html
    }

    fn diagnostics(rtf: &[u8]) -> Diagnostics {
        deencapsulate_html(rtf, Limits::default())
            .expect("encapsulated html")
            .diagnostics
    }

    #[test]
    fn recognizes_the_four_kinds() {
        assert_eq!(
            recognize(br"{\rtf1\ansi\ansicpg1252\fromhtml1 \fbidis \deff0{\fonttbl}"),
            RtfKind::EncapsulatedHtml
        );
        assert_eq!(
            recognize(br"{\rtf1\ansi\fromtext \deff0}"),
            RtfKind::EncapsulatedText
        );
        assert_eq!(recognize(br"{\rtf1\ansi Hello}"), RtfKind::PlainRtf);
        assert_eq!(recognize(b"Hello"), RtfKind::NotRtf);
        assert_eq!(recognize(b""), RtfKind::NotRtf);
        assert_eq!(recognize(br"{\rtf2\fromhtml1 }"), RtfKind::NotRtf);
    }

    #[test]
    fn recognition_inspects_only_the_first_ten_tokens() {
        // `\fromhtml1` is exactly the 10th token: found.
        assert_eq!(
            recognize(br"{\rtf1\a\b\c\d\e\g\h\fromhtml1 }"),
            RtfKind::EncapsulatedHtml
        );
        // The 11th: not looked at.
        assert_eq!(
            recognize(br"{\rtf1\a\b\c\d\e\g\h\i\fromhtml1 }"),
            RtfKind::PlainRtf
        );
    }

    #[test]
    fn recognition_stops_at_any_token_that_is_not_a_group_start_or_a_control_word() {
        assert_eq!(recognize(br"{\rtf1 Hello\fromhtml1 }"), RtfKind::PlainRtf);
        assert_eq!(
            recognize(br"{\rtf1\ansi{\fonttbl}\fromhtml1 }"),
            RtfKind::PlainRtf
        );
        // `\fromhtml` with another parameter is not the FROMHTML control word.
        assert_eq!(recognize(br"{\rtf1\fromhtml0 }"), RtfKind::PlainRtf);
    }

    #[test]
    fn tags_and_text_are_recovered_in_order() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl{\f0\fswiss\fcharset0 Arial;}}{\*\htmltag64}{\*\htmltag84 <b>}Hello{\*\htmltag92 </b>}}";
        assert_eq!(html(rtf), "<b>Hello</b>");
        let d = diagnostics(rtf);
        assert_eq!(d.htmltag_groups, 3);
        assert_eq!(d.fonts_defined, 1);
        assert_eq!(d.default_codepage, 1252);
        assert!(d.default_codepage_declared);
    }

    #[test]
    fn text_suppressed_by_htmlrtf_is_dropped_and_the_toggle_is_group_scoped() {
        let rtf = br"{\rtf1\ansi\fromhtml1 {\*\htmltag64}\htmlrtf {\b junk}\htmlrtf0 text}";
        assert_eq!(html(rtf), "text");
        assert_eq!(diagnostics(rtf).htmlrtf_regions, 1);

        // An `\htmlrtf` inside a group ends with the group.
        let rtf = br"{\rtf1\ansi\fromhtml1 {\*\htmltag64}{\htmlrtf junk}kept}";
        assert_eq!(html(rtf), "kept");
    }

    #[test]
    fn htmltag_content_is_copied_even_inside_an_htmlrtf_region() {
        let rtf = br"{\rtf1\ansi\fromhtml1 {\*\htmltag64}\htmlrtf {\*\htmltag84 <i>}x\htmlrtf0 }";
        assert_eq!(html(rtf), "<i>");
    }

    #[test]
    fn paragraph_line_and_tab_words_become_characters() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}a\par b\line c\tab d}";
        assert_eq!(html(rtf), "a\r\nb\r\nc\td");
        let d = diagnostics(rtf);
        assert_eq!(d.paragraph_breaks, 2);
        assert_eq!(d.tabs, 1);
    }

    #[test]
    fn a_backslash_before_a_line_break_is_a_paragraph_break() {
        let rtf = b"{\\rtf1\\fromhtml1 {\\*\\htmltag64}a\\\r\nb}";
        assert_eq!(html(rtf), "a\r\nb");
    }

    #[test]
    fn hex_escapes_use_the_code_page() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 {\*\htmltag64}caf\'e9}";
        assert_eq!(html(rtf), "caf\u{e9}");
        let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 {\*\htmltag64}{\*\htmltag84 caf\'e9}}";
        assert_eq!(html(rtf), "caf\u{e9}");
        assert_eq!(diagnostics(rtf).hex_escapes, 1);
    }

    #[test]
    fn a_bad_hex_escape_is_counted_not_fatal() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}a\'zzb}";
        let d = diagnostics(rtf);
        assert_eq!(d.bad_hex_escapes, 1);
        assert!(html(rtf).starts_with('a'));
    }

    #[test]
    fn unicode_escapes_skip_their_fallback_characters() {
        assert_eq!(
            html(br"{\rtf1\fromhtml1 {\*\htmltag64}\u8364? x}"),
            "\u{20AC} x"
        );
        assert_eq!(
            html(br"{\rtf1\fromhtml1 {\*\htmltag64}\uc0\u8364 x}"),
            "\u{20AC}x"
        );
        // A fallback written as a hex escape counts as one character.
        assert_eq!(
            html(br"{\rtf1\fromhtml1 {\*\htmltag64}\u8364\'80x}"),
            "\u{20AC}x"
        );
        // Values above 32767 are written negative.
        assert_eq!(
            html(br"{\rtf1\fromhtml1 {\*\htmltag64}\u-3913?}"),
            "\u{F0B7}"
        );
    }

    #[test]
    fn surrogate_pairs_combine_and_unpaired_halves_are_replaced() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\u-10179?\u-8704?}";
        assert_eq!(html(rtf), "\u{1F600}");

        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\u-10179?x}";
        assert_eq!(html(rtf), "\u{FFFD}x");
        assert_eq!(diagnostics(rtf).unpaired_surrogates, 1);

        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\u-8704?}";
        assert_eq!(html(rtf), "\u{FFFD}");
    }

    #[test]
    fn ignorable_and_standard_destinations_are_skipped() {
        let rtf = br"{\rtf1\ansi\fromhtml1 {\colortbl;\red0\green0\blue0;}{\*\generator Msftedit;}{\*\htmltag64}{\*\mhtmltag64 <skip>}shown}";
        assert_eq!(html(rtf), "shown");
        let d = diagnostics(rtf);
        assert_eq!(d.standard_destinations_skipped, 1);
        assert_eq!(d.ignorable_destinations_skipped, 2);
        assert_eq!(d.htmltag_groups, 1);
    }

    #[test]
    fn nested_groups_inside_a_skipped_destination_stay_skipped() {
        let rtf =
            br"{\rtf1\ansi\fromhtml1 {\*\htmltag64}{\*\other {\*\htmltag84 <nope>} inner}after}";
        assert_eq!(html(rtf), "after");
    }

    #[test]
    fn object_groups_are_skipped_and_counted() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}a{\object\objemb{\*\objclass X}}b}";
        assert_eq!(html(rtf), "ab");
        assert_eq!(diagnostics(rtf).object_placeholders, 1);
    }

    #[test]
    fn binary_data_is_skipped_by_length() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}a{\pict\bin3 {}}}b}";
        assert_eq!(html(rtf), "ab");
        assert_eq!(diagnostics(rtf).binary_bytes_skipped, 3);
        // A length beyond the end of the input is clamped, not a panic.
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}a{\pict\bin99999 ";
        assert_eq!(html(rtf), "a");
    }

    #[test]
    fn escaped_braces_and_backslashes_are_literal_text() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\{a\}\\b}";
        assert_eq!(html(rtf), r"{a}\b");
    }

    #[test]
    fn unicode_control_words_and_symbols_are_converted() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\lquote a\rquote \emdash\~b}";
        assert_eq!(html(rtf), "\u{2018}a\u{2019}\u{2014}\u{00A0}b");
    }

    #[test]
    fn other_control_words_are_ignored() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}\pard\plain\fs20\b bold\b0 }";
        assert_eq!(html(rtf), "bold");
    }

    #[test]
    fn a_font_selects_the_code_page_for_ordinary_text() {
        // Windows-1252 font: decoded.
        let rtf = br"{\rtf1\ansi\fromhtml1 {\fonttbl{\f1\fcharset0 Arial;}}{\*\htmltag64}\f1 \'e9}";
        assert_eq!(html(rtf), "\u{e9}");
        // A code page the project's decoders do not implement: counted, never guessed.
        let rtf =
            br"{\rtf1\ansi\fromhtml1 {\fonttbl{\f1\fcharset204 Arial;}}{\*\htmltag64}\f1 \'ef}";
        let result = deencapsulate_html(rtf, Limits::default()).unwrap();
        assert_eq!(result.html, "\u{FFFD}");
        assert_eq!(result.diagnostics.undecodable_bytes, 1);
        assert!(result.diagnostics.unsupported_codepages.contains(&1251));
    }

    #[test]
    fn the_font_is_tracked_even_where_text_is_suppressed() {
        let rtf = br"{\rtf1\ansi\fromhtml1 {\fonttbl{\f1\fcharset0 Arial;}}{\*\htmltag64}\htmlrtf\f1 \htmlrtf0 \'e9}";
        assert_eq!(html(rtf), "\u{e9}");
    }

    #[test]
    fn htmltag_content_uses_the_default_code_page_not_the_font() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 {\fonttbl{\f1\fcharset204 Arial;}}{\*\htmltag64}\f1 {\*\htmltag84 \'e9}}";
        assert_eq!(html(rtf), "\u{e9}");
    }

    #[test]
    fn a_stale_charset_declaration_is_flagged() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}{\*\htmltag84 <meta charset=windows-1252>}}";
        assert!(diagnostics(rtf).meta_charset_declared);
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}{\*\htmltag84 <p>}}";
        assert!(!diagnostics(rtf).meta_charset_declared);
    }

    #[test]
    fn documents_that_are_not_encapsulated_html_are_refused_with_their_kind() {
        assert_eq!(
            deencapsulate_html(br"{\rtf1\ansi Hello}", Limits::default()).err(),
            Some(RtfKind::PlainRtf)
        );
        assert_eq!(
            deencapsulate_html(br"{\rtf1\fromtext Hello}", Limits::default()).err(),
            Some(RtfKind::EncapsulatedText)
        );
        assert_eq!(
            deencapsulate_html(b"plain", Limits::default()).err(),
            Some(RtfKind::NotRtf)
        );
    }

    #[test]
    fn deep_nesting_stops_at_the_limit_without_panicking() {
        let mut rtf = b"{\\rtf1\\fromhtml1 ".to_vec();
        rtf.extend(std::iter::repeat_n(b'{', 2000));
        let limits = Limits {
            max_depth: 100,
            max_output_bytes: 1024,
        };
        let result = deencapsulate_html(&rtf, limits).unwrap();
        assert!(result.diagnostics.depth_limit_hit);
    }

    #[test]
    fn output_stops_at_the_limit() {
        let rtf = br"{\rtf1\fromhtml1 {\*\htmltag64}0123456789abcdef}";
        let limits = Limits {
            max_depth: 64,
            max_output_bytes: 10,
        };
        let result = deencapsulate_html(rtf, limits).unwrap();
        assert!(result.diagnostics.output_limit_hit);
        assert!(result.html.len() <= 10);
    }

    #[test]
    fn unclosed_and_stray_groups_are_counted() {
        let d = diagnostics(br"{\rtf1\fromhtml1 {\*\htmltag64}a{b");
        assert_eq!(d.unclosed_groups_at_end, 2);
        let d = diagnostics(br"{\rtf1\fromhtml1 }}}");
        assert_eq!(d.stray_group_ends, 2);
    }

    #[test]
    fn a_trailing_backslash_is_harmless() {
        assert_eq!(html(b"{\\rtf1\\fromhtml1 {\\*\\htmltag64}a\\"), "a");
    }

    #[test]
    fn a_trailing_backslash_is_dropped_but_a_escaped_one_is_not() {
        // A lone trailing backslash is malformed and produces nothing.
        assert_eq!(html(b"{\\rtf1\\fromhtml1 {\\*\\htmltag64}a\\"), "a");
        // An escaped backslash at end of input is a literal backslash.
        assert_eq!(html(b"{\\rtf1\\fromhtml1 {\\*\\htmltag64}a\\\\"), "a\\");
    }

    proptest! {
        #[test]
        fn arbitrary_input_never_panics_and_respects_the_bound(
            tail in proptest::collection::vec(any::<u8>(), 0..1024)
        ) {
            let mut rtf = b"{\\rtf1\\fromhtml1 ".to_vec();
            rtf.extend_from_slice(&tail);
            let limits = Limits { max_depth: 64, max_output_bytes: 4096 };
            let result = deencapsulate_html(&rtf, limits).expect("the prefix is recognized");
            prop_assert!(result.html.len() <= 4096);
        }

        #[test]
        fn recognition_never_panics_on_arbitrary_bytes(
            data in proptest::collection::vec(any::<u8>(), 0..512)
        ) {
            let _ = recognize(&data);
        }
    }
}
