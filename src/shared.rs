//! Shared vocabulary and counters used by the PST diagnostic, the MSG report, and the custom
//! MS-OXMSG parser: the MS-OXRTFEX encapsulated-HTML check, MAPI property constants, and the
//! presence/availability counters.

/// A generic MS-CFB container opened from a file on disk.
pub(crate) type CompoundFile = cfb::CompoundFile<std::fs::File>;

/// Deepest directory nesting an input scan descends into (guards against link loops). Directories
/// below it are not entered; they are counted, never silently skipped.
pub(crate) const MAX_DIRECTORY_DEPTH: usize = 64;

/// Deepest PST folder nesting walked (guards against a corrupt or cyclic folder hierarchy).
/// Subfolders below it are not opened; each one is counted as a folder open error.
pub(crate) const MAX_FOLDER_DEPTH: usize = 128;

/// True for a path whose extension is `.msg`, in any letter case. Extension only: nothing is read.
pub(crate) fn is_msg_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("msg"))
}

/// Serializes the temporary silencing of the panic hook (see [`run_contained`]), so concurrent
/// callers (tests run in parallel) cannot restore each other's hooks out of order.
static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `f` and returns its result, or `None` if it panicked. While it runs the panic hook is
/// silenced: a panic message could quote the content being processed, and tsp never prints
/// content. A panic on a thread that `f` spawns and joins is silenced too (the caller sees the
/// failed join). This contains a panic; it does not contain a stack overflow, which aborts the
/// process, so recursion on untrusted nesting has to be bounded before calling.
pub(crate) fn run_contained<T>(f: impl FnOnce() -> T) -> Option<T> {
    let _guard = PANIC_HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous_hook);
    outcome.ok()
}

// =============================================================================
// Shared: MS-OXRTFEX "RTF containing encapsulated HTML" detection
// =============================================================================
//
// Both the PST and MSG paths must apply identically strict detection rather
// than two signals that can silently drift apart. Neither the `msg_parser`
// nor `outlook_pst` convenience "give me HTML" method gates on this control
// word (proven against `RTF_message.msg`, a genuinely RTF-authored fixture),
// so both diagnostics check for the literal control word in the decompressed
// RTF bytes directly. See README "Change history" for the full narrative.

/// The MS-OXRTFEX control word a de-encapsulating reader uses to recognize
/// RTF containing encapsulated HTML. Per that specification, a reader
/// finding this control word SHOULD conclude the RTF document contains
/// encapsulated HTML and stop further inspection -- its presence alone is
/// the specification-sanctioned signal, not a heuristic.
pub(crate) const FROMHTML_MARKER: &[u8] = b"\\fromhtml1";

/// Returns whether decompressed RTF bytes contain the FROMHTML control
/// word. A plain byte search, not a UTF-8/String conversion: RTF is an
/// ASCII-based control-word format (non-ASCII text is escaped as `\'XX`
/// hex sequences), so searching raw bytes avoids encoding questions.
pub(crate) fn rtf_bytes_contain_fromhtml(rtf_bytes: &[u8]) -> bool {
    rtf_bytes
        .windows(FROMHTML_MARKER.len())
        .any(|window| window == FROMHTML_MARKER)
}

/// The result of checking compressed RTF bytes for MS-OXRTFEX HTML
/// encapsulation. Distinct variants (rather than a single bool) keep the
/// common unremarkable cases separate from genuine anomalies, consistent
/// with the project's no-silent-loss principle.
pub(crate) enum RtfHtmlCheck {
    NoRtfProperty,
    NotBinary,
    DecompressionFailed,
    Decompressed {
        contains_fromhtml: bool,
        decompressed_bytes: usize,
    },
}

/// The largest decompressed RTF accepted: 256 MiB. A document whose header declares more is
/// refused before decompression starts, and counts as a decompression failure like any other.
pub(crate) const MAX_DECOMPRESSED_RTF_BYTES: usize = 256 * 1024 * 1024;

/// Decompresses raw `PidTagRtfCompressed` bytes (MS-OXRTFCP) with `compressed-rtf`, or `None` when
/// they cannot be. The one place in tsp that calls the crate, so the three guards it needs are in
/// one place:
///
/// - `compressed_rtf::decompress_rtf` unconditionally indexes the first 16 bytes of its input (its
///   MS-OXRTFCP header read) and panics rather than erroring if given fewer (confirmed by reading
///   the crate's source), so shorter input is refused first;
/// - the header's raw size (bytes 4 to 8, little-endian) is checked against
///   [`MAX_DECOMPRESSED_RTF_BYTES`] before decompression, and the result after it;
/// - a panic inside the crate on corrupt input is contained ([`run_contained`]) and reported as a
///   failure, not allowed to take the process down.
///
/// The decompressed bytes are the caller's to use; nothing here prints them.
pub(crate) fn decompress_rtf_bounded(compressed: &[u8]) -> Option<Vec<u8>> {
    let header: [u8; 16] = compressed.get(..16)?.try_into().ok()?;
    let raw_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if usize::try_from(raw_size).is_ok_and(|size| size > MAX_DECOMPRESSED_RTF_BYTES) {
        return None;
    }
    let rtf = run_contained(|| compressed_rtf::decompress_rtf(compressed).ok())??;
    if rtf.len() > MAX_DECOMPRESSED_RTF_BYTES {
        return None;
    }
    Some(rtf.as_bytes().to_vec())
}

/// Checks raw `PidTagRtfCompressed` bytes for MS-OXRTFEX HTML
/// encapsulation (see [`FROMHTML_MARKER`]). Decompression uses
/// `compressed-rtf` (MS-OXRTFCP), through [`decompress_rtf_bounded`]; its
/// magic numbers and dictionary were independently cross-checked against
/// `msg_parser`'s own implementation and match. Never returns or exposes
/// the actual RTF or HTML content.
pub(crate) fn check_compressed_rtf_bytes(compressed: &[u8]) -> RtfHtmlCheck {
    match decompress_rtf_bounded(compressed) {
        Some(rtf) => RtfHtmlCheck::Decompressed {
            contains_fromhtml: rtf_bytes_contain_fromhtml(&rtf),
            decompressed_bytes: rtf.len(),
        },
        None => RtfHtmlCheck::DecompressionFailed,
    }
}

// =============================================================================
// Shared: MAPI property vocabulary (PST, MSG, and custom-OXMSG paths alike)
// =============================================================================

/// MS-OXPROPS property identifiers used only to check *presence* or read a
/// small, bounded integer/enum value -- never to read or print free-form
/// content (names, addresses, filenames).
pub(crate) const PROP_BODY: u16 = 0x1000; // PidTagBody (plain text body)
pub(crate) const PROP_BODY_HTML: u16 = 0x1013; // PidTagBodyHtml
pub(crate) const PROP_RTF_COMPRESSED: u16 = 0x1009; // PidTagRtfCompressed
pub(crate) const PROP_RECIPIENT_TYPE: u16 = 0x0C15; // PidTagRecipientType
pub(crate) const PROP_ATTACH_SIZE: u16 = 0x0E20; // PidTagAttachSize
pub(crate) const PROP_ATTACH_METHOD: u16 = 0x3705; // PidTagAttachMethod
pub(crate) const PROP_ATTACH_CONTENT_ID: u16 = 0x3712; // PidTagAttachContentId
pub(crate) const PROP_ATTACH_DATA_BINARY: u16 = 0x3701; // PidTagAttachDataBinary

/// PidTagRecipientType values (MS-OXOMSG).
pub(crate) const RECIPIENT_TYPE_ORIG: i32 = 0;
pub(crate) const RECIPIENT_TYPE_TO: i32 = 1;
pub(crate) const RECIPIENT_TYPE_CC: i32 = 2;
pub(crate) const RECIPIENT_TYPE_BCC: i32 = 3;

/// PidTagAttachMethod values (MS-OXCMSG).
pub(crate) const ATTACH_METHOD_NONE: i32 = 0;
pub(crate) const ATTACH_METHOD_BY_VALUE: i32 = 1;
pub(crate) const ATTACH_METHOD_BY_REFERENCE: i32 = 2;
pub(crate) const ATTACH_METHOD_BY_REFERENCE_RESOLVE: i32 = 3;
pub(crate) const ATTACH_METHOD_BY_REFERENCE_ONLY: i32 = 4;
pub(crate) const ATTACH_METHOD_EMBEDDED_MESSAGE: i32 = 5;
pub(crate) const ATTACH_METHOD_OLE: i32 = 6;

// =============================================================================
// Shared: presence/availability counters used by both the PST and MSG
// diagnostics
//
// Both diagnostics count through the same code so the format adapters
// cannot silently drift apart. Only the bucketing that genuinely differs
// (PST reads PidTagRecipientType / PidTagAttachMethod as table columns;
// msg_parser exposes its own vocabulary) remains format-specific.
// =============================================================================

/// Body-*availability* counters shared by the PST and MSG diagnostics.
/// All recording goes through [`BodyCounters::record`], which receives
/// presence booleans only -- never actual body content.
///
/// Interpretation caveat: `plain` reflects only that `PidTagBody` exists,
/// not that the message was *authored* in plain text. Outlook commonly
/// populates a plain-text compatibility mirror alongside an HTML- or
/// RTF-authored body, so `plain` runs high even on mailboxes with little
/// genuinely plain-text-only content (confirmed against this project's
/// PST fixture).
///
/// `html_native` and `html_via_rtf` are tracked as distinct signals, never
/// silently merged: many real messages have no native PidTagBodyHtml at
/// all -- Outlook encapsulates the HTML inside PidTagRtfCompressed per
/// MS-OXRTFEX, detectable via the FROMHTML control word. A single merged
/// `html` counter alone would have undercounted real HTML content.
#[derive(Default)]
pub(crate) struct BodyCounters {
    pub(crate) plain: u64,
    pub(crate) html: u64,
    pub(crate) html_native: u64,
    pub(crate) html_via_rtf: u64,
    pub(crate) rtf: u64,
    pub(crate) rtf_decompression_errors: u64,
    /// Sum of decompressed-RTF byte lengths across every message where
    /// decompression succeeded (regardless of whether the FROMHTML marker
    /// was found) -- a size-only diagnostic so the PST and MSG sides can be
    /// compared without ever printing content.
    pub(crate) rtf_decompressed_bytes_total: u64,
}

impl BodyCounters {
    /// Records body-*availability* only. Never receives or touches actual
    /// body content -- callers must pass presence booleans.
    pub(crate) fn record(
        &mut self,
        has_plain: bool,
        has_html_native: bool,
        has_html_via_rtf: bool,
        has_rtf: bool,
    ) {
        if has_plain {
            self.plain += 1;
        }
        if has_html_native {
            self.html_native += 1;
        }
        if has_html_via_rtf {
            self.html_via_rtf += 1;
        }
        if has_html_native || has_html_via_rtf {
            self.html += 1;
        }
        if has_rtf {
            self.rtf += 1;
        }
    }

    pub(crate) fn note_decompressed_bytes(&mut self, decompressed_bytes: usize) {
        self.rtf_decompressed_bytes_total += decompressed_bytes as u64;
    }

    pub(crate) fn note_decompression_error(&mut self) {
        self.rtf_decompression_errors += 1;
    }
}

/// The messages-with-any / total / max-on-a-message triple, shared by
/// recipient and attachment counting on both the PST and MSG sides.
#[derive(Default)]
pub(crate) struct CountStats {
    pub(crate) with_any: u64,
    pub(crate) total: u64,
    pub(crate) max: u64,
}

impl CountStats {
    pub(crate) fn record(&mut self, count: u64) {
        if count > 0 {
            self.with_any += 1;
        }
        self.total += count;
        self.max = self.max.max(count);
    }
}

/// Zero-size attachment bookkeeping, shared by the PST and MSG sides.
///
/// A zero `PidTagAttachSize` (PST) or zero `payload_bytes` (MSG) is only a
/// meaningful "empty file" signal when the attachment's method is
/// `by_value`. For every other method (embedded message, OLE,
/// by-reference), the attachment's real content lives outside that
/// property entirely, so a zero reading there is expected and structural,
/// not evidence of an empty file -- tracked separately rather than
/// silently merged into `attachments_zero_byte`. A missing or unreadable
/// size is not counted as zero-byte either way.
#[derive(Default)]
pub(crate) struct ZeroByteStats {
    pub(crate) by_value: u64,
    pub(crate) other_method: u64,
}

impl ZeroByteStats {
    /// `size_is_zero` must already be `false` for a missing or unreadable
    /// size; `is_by_value` should be `false` when the method is missing or
    /// is any non-by-value method.
    pub(crate) fn record(&mut self, size_is_zero: bool, is_by_value: bool) {
        if !size_is_zero {
            return;
        }
        if is_by_value {
            self.by_value += 1;
        } else {
            self.other_method += 1;
        }
    }
}
