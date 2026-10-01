//! Custom MS-OXMSG extraction layer: real property reads feeding the default `.msg` report and
//! `--verify`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::msg_report::{
    print_msg_report, record_embedded_message_class, record_msg_class, record_msg_recipients,
    MsgTotals, MSG_ATTACH_METHOD_EMBEDDED_MESSAGE, PROP_TYPE_ERROR, PROP_TYPE_UNSPECIFIED,
};
use crate::oxmsg_classify::{
    message_shaped_parent_paths, top_level_storage_paths, OxmsgEntryKind, OxmsgEntryScope,
    EMBEDDED_OBJECT_STORAGE_NAME,
};
use crate::oxmsg_decode::{
    decode_fixed_value, decode_properties_stream, decode_string8_with_codepage,
    decode_unicode_value, expected_variable_stream_path, extract_string8_codepage,
    properties_stream_header_len, read_stream_bytes, DecodedFixedValue, String8Decoded,
    PROP_MESSAGE_CLASS,
};
use crate::shared::{
    check_compressed_rtf_bytes, CompoundFile, RtfHtmlCheck, PROP_ATTACH_CONTENT_ID,
    PROP_ATTACH_METHOD, PROP_BODY, PROP_BODY_HTML, PROP_RECIPIENT_TYPE, PROP_RTF_COMPRESSED,
};

// =============================================================================
// Custom MS-OXMSG extraction layer: real property reads used by the default
// .msg report and by --verify
//
// Unlike the structural accounting, these functions return real content.
// They are only ever consumed by the default report (which prints only
// counters) and --verify (which prints only match/mismatch counts) --
// never printed directly.
// =============================================================================

/// Reads a string property by ID from `parent`: PT_UNICODE if present,
/// otherwise PT_STRING8 decoded under `string8_codepage`. `None` means
/// absent, undecodable, or under a code page this decoder does not
/// implement (never silently decoded as something else).
pub(crate) fn read_string_property(
    comp: &mut CompoundFile,
    parent: &Path,
    property_id: u16,
    string8_codepage: u32,
) -> Option<String> {
    if let Some(bytes) = read_stream_bytes(
        comp,
        &expected_variable_stream_path(parent, property_id, 0x001F),
    ) {
        return decode_unicode_value(&bytes).ok();
    }
    let bytes = read_stream_bytes(
        comp,
        &expected_variable_stream_path(parent, property_id, 0x001E),
    )?;
    match decode_string8_with_codepage(&bytes, string8_codepage) {
        String8Decoded::Decoded { text, .. }
        | String8Decoded::AsciiUnderUnsupportedCodepage { text } => Some(text),
        String8Decoded::UnsupportedCodepage => None,
    }
}

/// Reads PidTagMessageClass (0x001A) directly from a message's own value
/// streams at the CFB root (PT_UNICODE, or PT_STRING8 under the resolved
/// code page chain). Not independently unit-tested for the UNICODE path:
/// it is a thin composition of already-tested pieces -- its real
/// verification is `run_msg_verify` producing a clean run against the
/// fixture corpus; the STRING8 path is covered by synthetic-fixture tests.
pub(crate) fn extract_message_class(comp: &mut CompoundFile) -> Option<String> {
    let codepage = extract_string8_codepage(
        comp,
        Path::new("/"),
        properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32),
    );
    read_string_property(comp, Path::new("/"), PROP_MESSAGE_CLASS, codepage.codepage)
}

pub(crate) struct BodyFlags {
    pub(crate) has_plain: bool,
    pub(crate) has_html_native: bool,
    pub(crate) has_html_via_rtf: bool,
    pub(crate) has_rtf: bool,
    /// Mirrors [`RtfHtmlCheck::DecompressionFailed`] -- too short to be
    /// valid MS-OXRTFCP, or the crate itself returned an error.
    pub(crate) decompression_failed: bool,
    /// Byte length of the successfully-decompressed RTF, if decompression
    /// ran at all. `None` when there's no RTF or decompression failed --
    /// distinct from `Some(0)`, an empty-but-valid result.
    pub(crate) decompressed_rtf_len: Option<u64>,
}

/// Reads PidTagBody, PidTagBodyHtml, and PidTagRtfCompressed directly by
/// name, the custom-path equivalent of what `msg_parser`'s `Outlook`
/// exposes as `body`/`html`/`rtf_compressed`. Real content is read into
/// memory to run the HTML-in-RTF check, but nothing here is ever printed.
pub(crate) fn extract_body_flags(comp: &mut CompoundFile) -> BodyFlags {
    let root = Path::new("/");
    // Non-empty, not merely present: msg_parser's `body`/`html`/
    // `rtf_compressed` are empty for a zero-length (or absent) value, and
    // the `!is_empty()` checks this path is diffed against in
    // --verify uses exactly that definition. A zero-length
    // stream exists in the CFB but carries no body, so `Some(vec![])`
    // must count as "no body" -- `.is_some()` alone made the two paths
    // disagree on any file with an empty body stream, and made an empty
    // RTF stream additionally report a phantom decompression error.
    let has_plain = read_stream_bytes(
        comp,
        &expected_variable_stream_path(root, PROP_BODY, 0x001F),
    )
    .or_else(|| {
        read_stream_bytes(
            comp,
            &expected_variable_stream_path(root, PROP_BODY, 0x001E),
        )
    })
    .is_some_and(|bytes| !bytes.is_empty());
    let has_html_native = read_stream_bytes(
        comp,
        &expected_variable_stream_path(root, PROP_BODY_HTML, 0x0102),
    )
    .is_some_and(|bytes| !bytes.is_empty());
    let rtf_bytes = read_stream_bytes(
        comp,
        &expected_variable_stream_path(root, PROP_RTF_COMPRESSED, 0x0102),
    )
    .filter(|bytes| !bytes.is_empty());
    let has_rtf = rtf_bytes.is_some();

    let mut decompression_failed = false;
    let mut decompressed_rtf_len = None;
    let has_html_via_rtf = if has_html_native {
        false
    } else if let Some(compressed) = &rtf_bytes {
        match check_compressed_rtf_bytes(compressed) {
            RtfHtmlCheck::Decompressed {
                contains_fromhtml,
                decompressed_bytes,
            } => {
                decompressed_rtf_len = Some(decompressed_bytes as u64);
                contains_fromhtml
            }
            RtfHtmlCheck::DecompressionFailed => {
                decompression_failed = true;
                false
            }
            RtfHtmlCheck::NoRtfProperty | RtfHtmlCheck::NotBinary => false,
        }
    } else {
        false
    };

    BodyFlags {
        has_plain,
        has_html_native,
        has_html_via_rtf,
        has_rtf,
        decompression_failed,
        decompressed_rtf_len,
    }
}

#[derive(Default)]
pub(crate) struct RecipientTypeCounts {
    /// MS-OXOMSG value 0 -- the sender, recorded as a recipient.
    /// `msg_parser`'s `Outlook` has no field for this at all; there is
    /// nothing to compare it against, only to report.
    pub(crate) orig: u64,
    pub(crate) to: u64,
    pub(crate) cc: u64,
    pub(crate) bcc: u64,
    /// A `PidTagRecipientType` value outside the four defined ones.
    pub(crate) other: u64,
    /// A recipient storage whose type couldn't be read at all (missing
    /// properties stream, or no `0x0C15` entry in it).
    pub(crate) unresolved: u64,
}

/// Walks every top-level `__recip_version1.0_#*` storage and classifies
/// each by its own `PidTagRecipientType` (0x0C15, PT_LONG) fixed-length
/// entry. Real content stays in memory only as counts by category --
/// never a recipient's actual address or name, which this function never
/// reads at all.
pub(crate) fn extract_recipient_type_counts(comp: &mut CompoundFile) -> RecipientTypeCounts {
    let mut counts = RecipientTypeCounts::default();

    // Pass 1 (immutable): every TOP-LEVEL recipient storage's path.
    let recipient_paths = top_level_storage_paths(&*comp, OxmsgEntryKind::RecipientStorage);

    // Pass 2 (mutable): read each one's own properties stream.
    for recip_path in recipient_paths {
        let properties_path = recip_path.join("__properties_version1.0");
        let Some(bytes) = read_stream_bytes(comp, &properties_path) else {
            counts.unresolved += 1;
            continue;
        };
        let Some(decoded) = decode_properties_stream(&bytes, 8) else {
            counts.unresolved += 1;
            continue;
        };
        let recipient_type = decoded.entries.iter().find_map(|entry| {
            if entry.property_id != PROP_RECIPIENT_TYPE || entry.property_type != 0x0003 {
                return None;
            }
            match decode_fixed_value(entry.property_type, &entry.tail) {
                Some(DecodedFixedValue::Long(value)) => Some(value),
                _ => None,
            }
        });
        match recipient_type {
            Some(0) => counts.orig += 1,
            Some(1) => counts.to += 1,
            Some(2) => counts.cc += 1,
            Some(3) => counts.bcc += 1,
            Some(_) => counts.other += 1,
            None => counts.unresolved += 1,
        }
    }

    counts
}

/// Counts top-level attachment storages only -- the same scoping as
/// `extract_recipient_type_counts`. `msg_parser` never opens an embedded
/// message, so its `outlook.attachments` never includes that message's
/// own attachments either.
pub(crate) fn extract_attachment_count(comp: &CompoundFile) -> u64 {
    top_level_storage_paths(comp, OxmsgEntryKind::AttachmentStorage).len() as u64
}

#[derive(Default)]
pub(crate) struct AttachmentMethodCounts {
    pub(crate) by_value: u64,
    pub(crate) embedded_message: u64,
    pub(crate) ole: u64,
    pub(crate) other: u64,
    pub(crate) with_content_id: u64,
    /// An attachment storage whose PidTagAttachMethod couldn't be read at
    /// all (missing properties stream, or no 0x3705 entry in it).
    /// `msg_parser` has no equivalent bucket -- it always reports some
    /// method value -- so this is reported on its own, not folded into
    /// `other`.
    pub(crate) unresolved: u64,
    /// A by-value attachment whose PidTagAttachDataBinary is present but
    /// empty. A data stream that is absent or unreadable is NOT counted
    /// here -- see `zero_data_stream_missing` -- consistent with the
    /// no-silent-loss rule on `ZeroByteStats::record`.
    pub(crate) zero_byte_by_value: u64,
    /// A by-value attachment whose PidTagAttachDataBinary stream could
    /// not be read at all (absent, or a CFB read error). Reported on its
    /// own rather than folded into `zero_byte_by_value`: an unreadable
    /// stream is an anomaly, not evidence of an empty file.
    pub(crate) zero_data_stream_missing: u64,
    /// Paths of the attachment storages confirmed (from their own
    /// PidTagAttachMethod) to hold embedded messages, so callers can open
    /// each one rather than just knowing that at least one exists.
    pub(crate) embedded_paths: Vec<PathBuf>,
    /// A non-by-value attachment (or one whose method couldn't be read).
    /// Mirrors `msg_parser`'s own structural behavior: it leaves
    /// `payload_bytes` empty for every method other than by-value, since
    /// OLE and embedded-message content lives in a storage, not a flat
    /// stream.
    pub(crate) zero_size_other_method: u64,
}

/// Walks every top-level attachment storage and classifies it by its own
/// `PidTagAttachMethod` (0x3705, PT_LONG, type-gated so a PT_ERROR
/// "not set" placeholder can't be misread as a method), plus whether
/// `PidTagAttachContentId` (0x3712) is present. Also collects the paths
/// of embedded-message attachments for per-attachment opening, and
/// distinguishes an empty PidTagAttachDataBinary payload (a genuine
/// zero-byte file) from one that could not be read at all. Real content
/// stays in memory only as counts by category -- never an attachment's
/// name or bytes, which this function never reads at all.
pub(crate) fn extract_attachment_method_counts(comp: &mut CompoundFile) -> AttachmentMethodCounts {
    let mut counts = AttachmentMethodCounts::default();

    let attachment_paths = top_level_storage_paths(&*comp, OxmsgEntryKind::AttachmentStorage);

    for attach_path in attachment_paths {
        let properties_path = attach_path.join("__properties_version1.0");
        let Some(bytes) = read_stream_bytes(comp, &properties_path) else {
            counts.unresolved += 1;
            continue;
        };
        let Some(decoded) = decode_properties_stream(&bytes, 8) else {
            counts.unresolved += 1;
            continue;
        };

        let mut method = None;
        let mut has_content_id = false;
        for entry in &decoded.entries {
            if entry.property_id == PROP_ATTACH_METHOD && entry.property_type == 0x0003 {
                if let Some(DecodedFixedValue::Long(value)) =
                    decode_fixed_value(entry.property_type, &entry.tail)
                {
                    method = Some(value);
                }
            } else if entry.property_id == PROP_ATTACH_CONTENT_ID
                && entry.property_type != PROP_TYPE_ERROR
                && entry.property_type != PROP_TYPE_UNSPECIFIED
            {
                // Type-gated, not ID-gated: MS-OXMSG represents a property
                // that is *not set* on the object as a PT_ERROR (0x000A)
                // entry carrying PidTagNotFound, so an attachment with no
                // content ID can still have a 0x3712 entry. Matching the
                // ID alone (the previous version) counted those
                // placeholders as "has content ID", inflating the count
                // relative to msg_parser's non-empty-`content_id` check.
                // PT_UNSPECIFIED (0x0000) is excluded the same way.
                has_content_id = true;
            }
        }

        match method {
            Some(1) => counts.by_value += 1,
            Some(5) => {
                counts.embedded_message += 1;
                counts.embedded_paths.push(attach_path.clone());
            }
            Some(6) => counts.ole += 1,
            Some(_) => counts.other += 1,
            None => counts.unresolved += 1,
        }
        if has_content_id {
            counts.with_content_id += 1;
        }

        let is_by_value = matches!(method, Some(1));
        if is_by_value {
            match read_stream_bytes(
                comp,
                &expected_variable_stream_path(&attach_path, 0x3701, 0x0102),
            ) {
                Some(data) if data.is_empty() => counts.zero_byte_by_value += 1,
                Some(_) => {}
                // Unreadable is not zero: the previous version's
                // `unwrap_or(true)` silently reported a missing or
                // corrupt data stream as an empty-file attachment,
                // violating the documented "a missing or unreadable size
                // is not counted as zero-byte" invariant.
                None => counts.zero_data_stream_missing += 1,
            }
        } else {
            counts.zero_size_other_method += 1;
        }
    }

    counts
}

/// One embedded message located and opened via CFB: its PidTagMessageClass
/// as a best-effort read. `class` is `None` when the message-shaped storage
/// exists but has no readable message-class stream -- which is NOT an open
/// failure; callers use the storage's presence as the success signal,
/// mirroring msg_parser, which opens such a message fine and simply
/// reports an empty class.
pub(crate) struct OpenedEmbeddedMessage {
    pub(crate) class: Option<String>,
}

/// Opens ONE embedded-message attachment, located from the attachment's
/// own properties rather than from an arbitrary member of
/// `message_shaped_parent_paths`.
///
/// Why not `.next()` on that set: it includes the top-level message
/// itself (`/`), every recipient storage, and every attachment storage --
/// all of which have their own `__properties_version1.0` stream. `/`
/// sorts first in a `BTreeSet<PathBuf>`, so the previous version of this
/// lookup read `/__substg1.0_001A001F` -- the outer message's own class
/// -- and reported it as the embedded message's. Instead, this version
/// confirms from each attachment's own `PidTagAttachMethod`
/// (type-gated against PT_ERROR placeholders) that it really is an
/// embedded-message attachment (method 5), checks the `3701000D` storage
/// under it is message-shaped, and only then reads the nested class
/// stream. Per-attachment rather than once per file, so a message with
/// several embedded-message attachments opens each one, matching
/// msg_parser's per-attachment `embedded_messages_opened` counting.
/// Never prints the class itself, only whether opening it succeeded.
pub(crate) fn open_embedded_message(
    comp: &mut CompoundFile,
    attach_path: &Path,
    message_shaped: &BTreeSet<PathBuf>,
) -> Option<OpenedEmbeddedMessage> {
    let properties_path = attach_path.join("__properties_version1.0");
    let bytes = read_stream_bytes(comp, &properties_path)?;
    let decoded = decode_properties_stream(&bytes, 8)?;
    let is_embedded_message = decoded.entries.iter().any(|entry| {
        entry.property_id == PROP_ATTACH_METHOD
            && entry.property_type == 0x0003
            // An equality comparison, not `matches!`: `as` casts are
            // expressions, and `matches!` takes patterns, in which
            // `MSG_ATTACH_METHOD_EMBEDDED_MESSAGE as i32` is a syntax
            // error rather than a cast.
            && decode_fixed_value(entry.property_type, &entry.tail)
                == Some(DecodedFixedValue::Long(
                    MSG_ATTACH_METHOD_EMBEDDED_MESSAGE as i32,
                ))
    });
    if !is_embedded_message {
        return None;
    }

    let embedded_path = attach_path.join(EMBEDDED_OBJECT_STORAGE_NAME);
    if !message_shaped.contains(&embedded_path) {
        // A custom/OLE payload storage, not a nested message: it cannot
        // be opened as an embedded message. Counted as a genuine open
        // error by the caller, not silently skipped.
        return None;
    }

    // Reaching here means the embedded message was located and its
    // storage opened. The class stream is best-effort: absent or
    // undecodable (including a PT_STRING8 variant this path doesn't
    // decode) still counts as a successfully opened message, just one
    // with no readable class -- same as msg_parser, which opens the
    // nested message and exposes an empty `message_class`.
    let embedded_codepage = extract_string8_codepage(
        comp,
        &embedded_path,
        properties_stream_header_len(OxmsgEntryScope::EmbeddedObject).unwrap_or(24),
    );
    let class = read_string_property(
        comp,
        &embedded_path,
        PROP_MESSAGE_CLASS,
        embedded_codepage.codepage,
    );

    Some(OpenedEmbeddedMessage { class })
}

/// Runs the custom extraction path alone -- no `msg_parser` at all -- and
/// prints the MSG inventory report (the default .msg path since M3f), plus one
/// custom-path-only anomaly key (attachments_data_stream_missing).
/// Its report shape was verified byte-identical to the former msg_parser
/// default report apart from that one extra key and the two triaged
/// improvements recorded in docs/verification/m3-results.md.
pub(crate) fn run_msg_extract(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    println!("inventory=privacy_safe");
    println!("input_kind=msg");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");

    let mut totals = MsgTotals::default();
    // Custom-path-only anomaly, reported alongside the totals rather than
    // merged into them: a by-value attachment whose data stream could not
    // be read. There is no MsgTotals field for it (msg_parser has no
    // equivalent signal), and the no-silent-loss rule forbids folding it
    // into a zero-byte count.
    let mut attachments_data_stream_missing = 0u64;

    for file in files {
        match cfb::open(file) {
            Ok(mut comp) => {
                inspect_oxmsg_as_msg(&mut comp, &mut totals, &mut attachments_data_stream_missing)
            }
            Err(_) => totals.open_errors += 1,
        }
    }

    print_msg_report(&totals);
    println!("attachments_data_stream_missing={attachments_data_stream_missing}");

    Ok(())
}

/// Populates the MSG report totals for one file: the exact same
/// `MsgTotals`, via the exact same shared `BodyCounters`/`CountStats` types
/// and `record_*` functions, sourced from the extraction primitives
/// verified by --verify field by field rather than from `msg_parser`'s
/// `Outlook`. Composes already-checked pieces rather than introducing new
/// logic. Every field here has a corresponding --verify result showing
/// zero mismatches, or, for embedded-message opening, a confirmed
/// improvement over `msg_parser`'s documented ceiling.
pub(crate) fn inspect_oxmsg_as_msg(
    comp: &mut CompoundFile,
    totals: &mut MsgTotals,
    attachments_data_stream_missing: &mut u64,
) {
    record_msg_class(totals, &extract_message_class(comp).unwrap_or_default());

    let body = extract_body_flags(comp);
    if body.decompression_failed {
        totals.bodies.note_decompression_error();
    } else if let Some(len) = body.decompressed_rtf_len {
        totals.bodies.note_decompressed_bytes(len as usize);
    }
    totals.bodies.record(
        body.has_plain,
        body.has_html_native,
        body.has_html_via_rtf,
        body.has_rtf,
    );

    let recipients = extract_recipient_type_counts(comp);
    record_msg_recipients(totals, recipients.to, recipients.cc, recipients.bcc);

    let methods = extract_attachment_method_counts(comp);
    totals.attachments_method_by_value += methods.by_value;
    totals.attachments_method_embedded_message += methods.embedded_message;
    totals.attachments_method_ole += methods.ole;
    totals.attachments_method_other += methods.other;
    totals.attachments_with_content_id += methods.with_content_id;
    for _ in 0..methods.zero_byte_by_value {
        totals.zero_byte_attachments.record(true, true);
    }
    for _ in 0..methods.zero_size_other_method {
        totals.zero_byte_attachments.record(true, false);
    }
    *attachments_data_stream_missing += methods.zero_data_stream_missing;
    totals.attachments.record(extract_attachment_count(&*comp));

    // Per embedded-message ATTACHMENT, not once per file: msg_parser's
    // diagnostic increments `embedded_messages_opened` for every
    // embedded-message attachment it opens, and the default report promises
    // to stay diffable against that report, so a message with two embedded
    // messages must report 2 here, not the previous version's 1.
    let message_shaped = message_shaped_parent_paths(&*comp);
    for attach_path in &methods.embedded_paths {
        match open_embedded_message(comp, attach_path, &message_shaped) {
            Some(opened) => {
                totals.embedded_messages_opened += 1;
                record_embedded_message_class(totals, opened.class.as_deref().unwrap_or(""));
            }
            None => totals.embedded_message_open_errors += 1,
        }
    }
}
