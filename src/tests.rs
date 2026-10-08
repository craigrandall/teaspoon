//! Unit tests (moved unchanged from the former single-file `main.rs`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Parser;
use outlook_pst::ltp::prop_context::PropertyValue;

use crate::cli::{Args, InputKind, classify_input};
use crate::msg_report::{
    MsgTotals, record_embedded_message_class, record_msg_class, record_msg_recipients,
};
use crate::oxmsg_classify::{
    CfbObjectKind, OxmsgEntryKind, OxmsgEntryScope, RecognizedNameTypeMismatch,
    UnrecognizedNameShape, classify_oxmsg_entry, enclosing_custom_payload_root,
    oxmsg_ancestry_shape, oxmsg_entry_scope, recognized_name_type_mismatch,
    unrecognized_name_shape,
};
use crate::oxmsg_decode::{
    CodepageSource, DecodedFixedValue, NamedPropertyMap, NamedPropertySet, PROP_INTERNET_CODEPAGE,
    PROP_MESSAGE_CODEPAGE, PSETID_COMMON, PropertyEntryShape, ResolvedCodepage, String8Decoded,
    classify_property_entry_shape, cp1252_to_char, decode_fixed_value, decode_named_property_entry,
    decode_named_property_string, decode_properties_stream, decode_string8_cp1252,
    decode_string8_with_codepage, decode_unicode_value, expected_size_field_value,
    expected_variable_stream_path, extract_string8_codepage, is_valid_boolean_encoding,
    properties_stream_header_len, resolve_string8_codepage,
};
use crate::oxmsg_extract::extract_message_class;
use crate::oxmsg_structure::{OxmsgTotals, print_structural_breakdown};
use crate::pst::{
    PstTotals, check_rtf_for_encapsulated_html, record_attachment_content_id_presence,
    record_attachment_method, record_message_class, record_recipient_type,
};
use crate::shared::{
    ATTACH_METHOD_BY_REFERENCE, ATTACH_METHOD_BY_REFERENCE_ONLY,
    ATTACH_METHOD_BY_REFERENCE_RESOLVE, ATTACH_METHOD_BY_VALUE, ATTACH_METHOD_EMBEDDED_MESSAGE,
    ATTACH_METHOD_NONE, ATTACH_METHOD_OLE, BodyCounters, CountStats, RECIPIENT_TYPE_BCC,
    RECIPIENT_TYPE_CC, RECIPIENT_TYPE_ORIG, RECIPIENT_TYPE_TO, RtfHtmlCheck, ZeroByteStats,
    rtf_bytes_contain_fromhtml,
};
use crate::verify::{
    BoolFieldComparison, CountComparison, MessageClassComparison, collect_msg_verify_totals,
    compare_bool_field, compare_count, compare_message_class, count_structural_gate_violations,
    structural_gate_values,
};

// --- Custom MS-OXMSG parser groundwork -----------------------------------

#[test]
fn oxmsg_entry_classification_covers_every_known_convention() {
    assert!(matches!(
        classify_oxmsg_entry("Root Entry", true),
        OxmsgEntryKind::Root
    ));
    assert!(matches!(
        classify_oxmsg_entry("__properties_version1.0", false),
        OxmsgEntryKind::PropertiesStream
    ));
    assert!(matches!(
        classify_oxmsg_entry("__nameid_version1.0", false),
        OxmsgEntryKind::NamedPropertyStorage
    ));
    assert!(matches!(
        classify_oxmsg_entry("__attach_version1.0_#00000000", false),
        OxmsgEntryKind::AttachmentStorage
    ));
    assert!(matches!(
        classify_oxmsg_entry("__recip_version1.0_#00000000", false),
        OxmsgEntryKind::RecipientStorage
    ));
    assert!(matches!(
        classify_oxmsg_entry("something_unexpected", false),
        OxmsgEntryKind::Unrecognized
    ));
}

#[test]
fn oxmsg_property_stream_name_decodes_property_id() {
    // __substg1.0_1000001F is PidTagBody (0x1000), PT_UNICODE (0x001F).
    // The classifier validates both 16-bit components as hexadecimal,
    // while the current diagnostic retains only the property ID.
    match classify_oxmsg_entry("__substg1.0_1000001F", false) {
        OxmsgEntryKind::PropertyStream { prop_id, indexed } => {
            assert_eq!(prop_id, 0x1000);
            assert!(!indexed);
        }
        _ => panic!("expected PropertyStream"),
    }
}

#[test]
fn oxmsg_indexed_property_stream_name_is_recognized() {
    match classify_oxmsg_entry("__substg1.0_6844101F-00000000", false) {
        OxmsgEntryKind::PropertyStream { prop_id, indexed } => {
            assert_eq!(prop_id, 0x6844);
            assert!(indexed);
        }
        _ => panic!("expected indexed PropertyStream"),
    }
}

#[test]
fn oxmsg_malformed_indexed_property_stream_name_is_unrecognized() {
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_6844101F-0000000", false),
        OxmsgEntryKind::Unrecognized
    ));
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_6844101F-0000000G", false),
        OxmsgEntryKind::Unrecognized
    ));
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_6844101F-00000000-extra", false),
        OxmsgEntryKind::Unrecognized
    ));
}

#[test]
fn oxmsg_embedded_object_storage_is_recognized() {
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_3701000D", false),
        OxmsgEntryKind::EmbeddedObjectStorage
    ));
}

#[test]
fn oxmsg_entry_scope_tracks_nearest_structural_container() {
    let message_properties = Path::new("Root Entry").join("__properties_version1.0");
    let attachment_properties = Path::new("Root Entry")
        .join("__attach_version1.0_#00000000")
        .join("__properties_version1.0");
    let recipient_property = Path::new("Root Entry")
        .join("__recip_version1.0_#00000000")
        .join("__substg1.0_0037001F");
    let embedded_properties = Path::new("Root Entry")
        .join("__attach_version1.0_#00000000")
        .join("__substg1.0_3701000D")
        .join("__properties_version1.0");
    let named_property_stream = Path::new("Root Entry")
        .join("__nameid_version1.0")
        .join("__substg1.0_00020102");

    assert_eq!(
        oxmsg_entry_scope(&message_properties),
        OxmsgEntryScope::Message
    );
    assert_eq!(
        oxmsg_entry_scope(&attachment_properties),
        OxmsgEntryScope::Attachment
    );
    assert_eq!(
        oxmsg_entry_scope(&recipient_property),
        OxmsgEntryScope::Recipient
    );
    assert_eq!(
        oxmsg_entry_scope(&embedded_properties),
        OxmsgEntryScope::EmbeddedObject
    );
    assert_eq!(
        oxmsg_entry_scope(&named_property_stream),
        OxmsgEntryScope::NamedPropertyStorage
    );
}

#[test]
fn oxmsg_malformed_substg_name_is_unrecognized_not_a_panic() {
    // Non-hex characters where a property ID/type should be.
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_ZZZZZZZZ", false),
        OxmsgEntryKind::Unrecognized
    ));
    // Wrong length (too short) for a valid PPPPTTTT suffix.
    assert!(matches!(
        classify_oxmsg_entry("__substg1.0_1000", false),
        OxmsgEntryKind::Unrecognized
    ));
}

#[test]
fn oxmsg_unrecognized_name_shapes_preserve_privacy_safe_structure() {
    assert_eq!(
        unrecognized_name_shape("__substg1.0_ZZZZZZZZ"),
        UnrecognizedNameShape::MalformedPropertyStream
    );
    assert_eq!(
        unrecognized_name_shape("__custom_version1.0"),
        UnrecognizedNameShape::OtherReserved
    );
    assert_eq!(
        unrecognized_name_shape("unexpected"),
        UnrecognizedNameShape::Other
    );
}

#[test]
fn oxmsg_known_names_with_wrong_cfb_object_types_are_reported() {
    assert_eq!(
        recognized_name_type_mismatch(
            &OxmsgEntryKind::PropertyStream {
                prop_id: 0x1000,
                indexed: false,
            },
            CfbObjectKind::Storage
        ),
        Some(RecognizedNameTypeMismatch::StreamNameIsStorage)
    );
    assert_eq!(
        recognized_name_type_mismatch(&OxmsgEntryKind::AttachmentStorage, CfbObjectKind::Stream),
        Some(RecognizedNameTypeMismatch::StorageNameIsStream)
    );
    assert_eq!(
        recognized_name_type_mismatch(&OxmsgEntryKind::RecipientStorage, CfbObjectKind::Storage),
        None
    );
    assert_eq!(
        recognized_name_type_mismatch(
            &OxmsgEntryKind::EmbeddedObjectStorage,
            CfbObjectKind::Stream
        ),
        Some(RecognizedNameTypeMismatch::StorageNameIsStream)
    );
}

#[test]
fn oxmsg_entry_accounting_gap_is_zero_when_all_entries_are_accounted_for() {
    let totals = OxmsgTotals {
        total_entries: 60,
        recognized_entries_total: 59,
        unrecognized_entries_total: 1,
        ..OxmsgTotals::default()
    };

    assert_eq!(totals.entry_accounting_gap_total(), 0);
}

#[test]
fn oxmsg_entry_accounting_gap_exposes_a_missing_entry() {
    let totals = OxmsgTotals {
        total_entries: 60,
        recognized_entries_total: 59,
        ..OxmsgTotals::default()
    };

    assert_eq!(totals.entry_accounting_gap_total(), 1);
}

#[test]
fn oxmsg_entry_accounting_gap_is_negative_when_categories_overcount() {
    let totals = OxmsgTotals {
        total_entries: 60,
        recognized_entries_total: 61,
        ..OxmsgTotals::default()
    };

    assert_eq!(totals.entry_accounting_gap_total(), -1);
}

#[test]
fn oxmsg_entry_category_totals_include_every_properties_stream() {
    let totals = OxmsgTotals {
        total_entries: 11,
        root_entries_total: 1,
        recognized_entries_total: 9,
        properties_stream_entries_total: 3,
        property_streams_total: 2,
        attachment_storages_total: 1,
        recipient_storages_total: 1,
        named_property_storages_total: 0,
        embedded_object_storages_total: 1,
        unrecognized_entries_total: 2,
        ..OxmsgTotals::default()
    };

    assert_eq!(
        totals.root_entries_total
            + totals.properties_stream_entries_total
            + totals.property_streams_total
            + totals.attachment_storages_total
            + totals.recipient_storages_total
            + totals.named_property_storages_total
            + totals.embedded_object_storages_total
            + totals.unrecognized_entries_total,
        totals.total_entries
    );
    assert_eq!(totals.entry_accounting_gap_total(), 0);
}

// --- Shared fromhtml-marker check ---------------------------------------

#[test]
fn fromhtml_marker_is_found_regardless_of_surrounding_bytes() {
    assert!(rtf_bytes_contain_fromhtml(
        b"{\\rtf1\\ansi\\fromhtml1 \\deff0{\\fonttbl}}"
    ));
    assert!(!rtf_bytes_contain_fromhtml(
        b"{\\rtf1\\ansi\\deff0{\\fonttbl}}"
    ));
    assert!(!rtf_bytes_contain_fromhtml(b""));
    // Shorter than the marker itself must not panic or false-positive.
    assert!(!rtf_bytes_contain_fromhtml(b"\\from"));
}

// --- PST-side tests (unchanged from v0.1.4.3, PstTotals renamed) -------

#[test]
fn message_class_is_aggregated_by_name() {
    let mut totals = PstTotals::default();
    record_message_class(&mut totals, Ok("IPM.Note".to_string()));
    record_message_class(&mut totals, Ok("IPM.Note".to_string()));
    record_message_class(&mut totals, Ok("IPM.Note.SMIME".to_string()));

    assert_eq!(totals.message_classes.get("IPM.Note"), Some(&2));
    assert_eq!(totals.message_classes.get("IPM.Note.SMIME"), Some(&1));
    assert_eq!(totals.message_class_read_errors, 0);
}

#[test]
fn unreadable_message_class_is_counted_not_dropped() {
    let mut totals = PstTotals::default();
    record_message_class(
        &mut totals,
        Err(std::io::Error::other("missing PidTagMessageClass")),
    );

    assert_eq!(totals.message_class_read_errors, 1);
    assert!(totals.message_classes.is_empty());
}

#[test]
fn body_flags_are_presence_only() {
    let mut bodies = BodyCounters::default();
    bodies.record(true, true, false, false);
    bodies.record(true, false, false, false);

    assert_eq!(bodies.plain, 2);
    assert_eq!(bodies.html, 1);
    assert_eq!(bodies.rtf, 0);
}

#[test]
fn html_native_and_via_rtf_are_tracked_separately_but_both_count_as_html() {
    let mut bodies = BodyCounters::default();
    // Native PidTagBodyHtml present.
    bodies.record(false, true, false, false);
    // No native property, but HTML recovered via MS-OXRTFEX
    // encapsulation in the RTF body -- the case the 2026-09-13 fix
    // exists for.
    bodies.record(false, false, true, true);

    assert_eq!(bodies.html_native, 1);
    assert_eq!(bodies.html_via_rtf, 1);
    assert_eq!(bodies.html, 2);
}

#[test]
fn rtf_html_check_distinguishes_absent_non_binary_and_decompression_failure() {
    // No RTF property at all -- the common, unremarkable case.
    assert!(matches!(
        check_rtf_for_encapsulated_html(None),
        RtfHtmlCheck::NoRtfProperty
    ));

    // Present but not a binary value -- shouldn't happen per spec, but
    // must not be misread as "no encapsulated HTML found".
    assert!(matches!(
        check_rtf_for_encapsulated_html(Some(&PropertyValue::Integer32(0))),
        RtfHtmlCheck::NotBinary
    ));

    // Present, binary, but shorter than the 16 bytes
    // compressed_rtf::decompress_rtf reads unconditionally as its own
    // header -- must be caught before ever calling that function, or
    // it panics rather than returning an error.
    let too_short =
        PropertyValue::Binary(outlook_pst::ltp::prop_context::BinaryValue::new(vec![0; 8]));
    assert!(matches!(
        check_rtf_for_encapsulated_html(Some(&too_short)),
        RtfHtmlCheck::DecompressionFailed
    ));

    // Present, binary, long enough, but not valid compressed RTF (a
    // bogus size header) -- a genuine anomaly, tracked separately from
    // "no encapsulated HTML found".
    let garbage = PropertyValue::Binary(outlook_pst::ltp::prop_context::BinaryValue::new(vec![
        0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]));
    assert!(matches!(
        check_rtf_for_encapsulated_html(Some(&garbage)),
        RtfHtmlCheck::DecompressionFailed
    ));
}

#[test]
fn count_stats_track_presence_total_and_max() {
    let mut recipients = CountStats::default();
    recipients.record(0);
    recipients.record(3);
    recipients.record(1);
    assert_eq!(recipients.with_any, 2);
    assert_eq!(recipients.total, 4);
    assert_eq!(recipients.max, 3);

    let mut attachments = CountStats::default();
    attachments.record(0);
    attachments.record(2);
    attachments.record(5);
    assert_eq!(attachments.with_any, 2);
    assert_eq!(attachments.total, 7);
    assert_eq!(attachments.max, 5);
}

#[test]
fn recipient_types_are_bucketed_correctly() {
    let mut totals = PstTotals::default();
    record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_ORIG));
    record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_TO));
    record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_TO));
    record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_CC));
    record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_BCC));
    record_recipient_type(&mut totals, Some(99));
    record_recipient_type(&mut totals, None);

    assert_eq!(totals.recipients_orig, 1);
    assert_eq!(totals.recipients_to, 2);
    assert_eq!(totals.recipients_cc, 1);
    assert_eq!(totals.recipients_bcc, 1);
    assert_eq!(totals.recipients_type_other, 1);
    assert_eq!(totals.recipients_type_unknown, 1);
}

#[test]
fn attachment_methods_are_bucketed_correctly() {
    let mut totals = PstTotals::default();
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_NONE));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_VALUE));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_REFERENCE));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_REFERENCE_RESOLVE));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_REFERENCE_ONLY));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_EMBEDDED_MESSAGE));
    record_attachment_method(&mut totals, Some(ATTACH_METHOD_OLE));
    record_attachment_method(&mut totals, Some(42));
    record_attachment_method(&mut totals, None);

    assert_eq!(totals.attachments_method_none, 1);
    assert_eq!(totals.attachments_method_by_value, 1);
    assert_eq!(totals.attachments_method_by_reference, 1);
    assert_eq!(totals.attachments_method_by_reference_resolve, 1);
    assert_eq!(totals.attachments_method_by_reference_only, 1);
    assert_eq!(totals.attachments_method_embedded_message, 1);
    assert_eq!(totals.attachments_method_ole, 1);
    assert_eq!(totals.attachments_method_other, 1);
    assert_eq!(totals.attachments_method_unknown, 1);
}

#[test]
fn zero_byte_attachments_are_counted_and_missing_size_is_not() {
    let mut zero = ZeroByteStats::default();
    // Some(0), by_value -> a genuine zero-byte file attachment.
    zero.record(true, true);
    // Some(1024) -> not zero.
    zero.record(false, true);
    // None (missing/unreadable size) -> not counted as zero-byte.
    zero.record(false, true);

    assert_eq!(zero.by_value, 1);
    assert_eq!(zero.other_method, 0);
}

#[test]
fn zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
    let mut zero = ZeroByteStats::default();
    zero.record(true, false); // embedded message
    zero.record(true, false); // OLE
    zero.record(true, false); // method missing entirely

    assert_eq!(zero.by_value, 0);
    assert_eq!(zero.other_method, 3);
}

#[test]
fn content_id_presence_is_counted_as_a_boolean_not_a_value() {
    let mut totals = PstTotals::default();
    record_attachment_content_id_presence(&mut totals, true);
    record_attachment_content_id_presence(&mut totals, false);
    record_attachment_content_id_presence(&mut totals, true);

    assert_eq!(totals.attachments_with_content_id, 2);
}

// --- MSG-side tests (new) -----------------------------------------------

#[test]
fn msg_class_is_aggregated_by_name_and_empty_is_counted_separately() {
    let mut totals = MsgTotals::default();
    record_msg_class(&mut totals, "IPM.Note");
    record_msg_class(&mut totals, "IPM.Note");
    record_msg_class(&mut totals, "");

    assert_eq!(totals.message_classes.get("IPM.Note"), Some(&2));
    assert_eq!(totals.message_class_missing, 1);
}

// The former msg_body_flags / msg_html_native_and_via_rtf tests are
// gone: record_msg_body_flags was merged into the shared BodyCounters,
// so those cases are covered once by the BodyCounters tests above.

#[test]
fn msg_recipients_are_split_by_type_with_presence_and_max() {
    let mut totals = MsgTotals::default();
    record_msg_recipients(&mut totals, 0, 0, 0);
    record_msg_recipients(&mut totals, 1, 2, 1);
    record_msg_recipients(&mut totals, 1, 0, 0);

    assert_eq!(totals.recipients.with_any, 2);
    assert_eq!(totals.recipients_to, 2);
    assert_eq!(totals.recipients_cc, 2);
    assert_eq!(totals.recipients_bcc, 1);
    assert_eq!(totals.recipients.max, 4);
}

#[test]
fn msg_zero_byte_attachment_is_counted() {
    let mut zero = ZeroByteStats::default();
    // msg_parser side: zero payload bytes on a by_value attachment.
    zero.record(true, true);
    zero.record(false, true);

    assert_eq!(zero.by_value, 1);
    assert_eq!(zero.other_method, 0);
}

#[test]
fn msg_zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
    let mut zero = ZeroByteStats::default();
    zero.record(true, false); // embedded message
    zero.record(true, false); // OLE

    assert_eq!(zero.by_value, 0);
    assert_eq!(zero.other_method, 2);
}

#[test]
fn msg_embedded_message_class_ignores_empty() {
    let mut totals = MsgTotals::default();
    record_embedded_message_class(&mut totals, "IPM.Note");
    record_embedded_message_class(&mut totals, "");

    assert_eq!(totals.embedded_message_classes.get("IPM.Note"), Some(&1));
    assert_eq!(totals.embedded_message_classes.len(), 1);
}

#[test]
fn oxmsg_entries_beneath_a_custom_embedded_object_storage_are_payload() {
    let payload = Path::new("/")
        .join("__attach_version1.0_#00000000")
        .join("__substg1.0_3701000D");
    let stream = payload.join("\u{1}CompObj");
    let message_shaped = BTreeSet::new();

    assert_eq!(
        enclosing_custom_payload_root(&stream, &message_shaped),
        Some(payload.clone())
    );
    // The storage itself keeps its own classification.
    assert_eq!(
        enclosing_custom_payload_root(&payload, &message_shaped),
        None
    );
}

#[test]
fn oxmsg_message_shaped_embedded_object_children_are_not_payload() {
    let payload = Path::new("/")
        .join("__attach_version1.0_#00000000")
        .join("__substg1.0_3701000D");
    let stream = payload.join("__substg1.0_1000001F");
    let mut message_shaped = BTreeSet::new();
    message_shaped.insert(payload);

    assert_eq!(
        enclosing_custom_payload_root(&stream, &message_shaped),
        None
    );
}

#[test]
fn oxmsg_ancestry_shape_uses_fixed_vocabulary_only() {
    let path = Path::new("/")
        .join("__attach_version1.0_#00000000")
        .join("__substg1.0_3701000D")
        .join("anything");
    assert_eq!(
        oxmsg_ancestry_shape(&path),
        "root/attachment/embedded_object"
    );
    assert_eq!(oxmsg_ancestry_shape(Path::new("/x")), "root");
}

#[test]
fn oxmsg_accounting_gap_counts_opaque_payload_as_accounted_for() {
    let totals = OxmsgTotals {
        total_entries: 10,
        recognized_entries_total: 6,
        opaque_payload_entries_total: 3,
        unrecognized_entries_total: 1,
        ..OxmsgTotals::default()
    };
    assert_eq!(totals.entry_accounting_gap_total(), 0);
}

#[test]
fn oxmsg_named_property_storage_streams_are_not_property_ids() {
    let path = Path::new("/")
        .join("__nameid_version1.0")
        .join("__substg1.0_10000102");
    assert_eq!(
        oxmsg_entry_scope(&path),
        OxmsgEntryScope::NamedPropertyStorage
    );
}

#[test]
fn oxmsg_property_entry_shape_classifies_fixed_variable_and_multivalued() {
    assert!(matches!(
        classify_property_entry_shape(0x0003), // PT_LONG
        PropertyEntryShape::FixedInline
    ));
    assert!(matches!(
        classify_property_entry_shape(0x001F), // PT_UNICODE
        PropertyEntryShape::VariableSingle
    ));
    assert!(matches!(
        classify_property_entry_shape(0x1003), // PT_MV_LONG
        PropertyEntryShape::VariableMultivalued
    ));
}

#[test]
fn oxmsg_properties_stream_header_len_matches_ms_oxmsg_2_4_1() {
    assert_eq!(
        properties_stream_header_len(OxmsgEntryScope::Message),
        Some(32)
    );
    assert_eq!(
        properties_stream_header_len(OxmsgEntryScope::EmbeddedObject),
        Some(24)
    );
    assert_eq!(
        properties_stream_header_len(OxmsgEntryScope::Attachment),
        Some(8)
    );
    assert_eq!(
        properties_stream_header_len(OxmsgEntryScope::Recipient),
        Some(8)
    );
    assert_eq!(
        properties_stream_header_len(OxmsgEntryScope::NamedPropertyStorage),
        None
    );
}

#[test]
fn oxmsg_decode_properties_stream_parses_entries_and_reports_trailing_bytes() {
    let mut bytes = vec![0u8; 8]; // an 8-byte (attachment/recipient) header
    // One PT_LONG (0x0003) entry, property ID 0x0E20, flags 0x01, value 7.
    bytes.extend_from_slice(&0x0003u16.to_le_bytes());
    bytes.extend_from_slice(&0x0E20u16.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&7i64.to_le_bytes());
    bytes.extend_from_slice(&[0xAA, 0xBB]); // trailing junk, not a full entry

    let decoded = decode_properties_stream(&bytes, 8).expect("decodes");
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].property_type, 0x0003);
    assert_eq!(decoded.entries[0].property_id, 0x0E20);
    assert_eq!(decoded.entries[0].flags, 1);
    assert_eq!(decoded.entries[0].tail, 7i64.to_le_bytes());
    assert_eq!(decoded.trailing_bytes, 2);
}

#[test]
fn oxmsg_decode_properties_stream_reports_too_short_for_header() {
    let bytes = vec![0u8; 4]; // shorter than any known header
    assert!(decode_properties_stream(&bytes, 8).is_none());
}

#[test]
fn oxmsg_boolean_encoding_validity() {
    assert!(is_valid_boolean_encoding(&[0, 0, 0, 0, 0, 0, 0, 0]));
    assert!(is_valid_boolean_encoding(&[1, 0, 0, 0, 0, 0, 0, 0]));
    assert!(!is_valid_boolean_encoding(&[2, 0, 0, 0, 0, 0, 0, 0]));
    assert!(!is_valid_boolean_encoding(&[1, 1, 0, 0, 0, 0, 0, 0]));
}

#[test]
fn oxmsg_expected_size_field_accounts_for_string_null_terminator_bytes() {
    assert_eq!(expected_size_field_value(0x001F, 10), 12); // PT_UNICODE: +2
    assert_eq!(expected_size_field_value(0x001E, 10), 11); // PT_STRING8: +1
    assert_eq!(expected_size_field_value(0x0102, 10), 10); // PT_BINARY: unchanged
}

#[test]
fn oxmsg_expected_variable_stream_path_matches_ms_oxmsg_naming() {
    let parent = Path::new("/");
    let path = expected_variable_stream_path(parent, 0x0037, 0x001F);
    assert_eq!(path, Path::new("/__substg1.0_0037001F"));
}

#[test]
fn oxmsg_named_property_entry_decodes_numeric_and_string_kind() {
    // Numeric: LID 0x0000811C, property index 5, guid index 4, kind 0.
    // Confirmed layout: high16 = property index, low16 = (guid_index << 1) | kind.
    let mut bytes = [0u8; 8];
    bytes[0..4].copy_from_slice(&0x0000_811Cu32.to_le_bytes());
    let low16 = 0x0004u32 << 1; // guid_index=4, kind=0
    let index_kind = low16 | (0x0005u32 << 16); // property_index=5
    bytes[4..8].copy_from_slice(&index_kind.to_le_bytes());
    let entry = decode_named_property_entry(&bytes);
    assert_eq!(entry.name_id_or_offset, 0x0000_811C);
    assert_eq!(entry.guid_index, 4);
    assert!(!entry.is_string);
    assert_eq!(entry.property_index, 5);

    // String: offset 0x10, property index 5, guid index 3, kind 1.
    let mut bytes = [0u8; 8];
    bytes[0..4].copy_from_slice(&0x0000_0010u32.to_le_bytes());
    let low16 = (0x0003u32 << 1) | 1; // guid_index=3, kind=1
    let index_kind = low16 | (0x0005u32 << 16); // property_index=5
    bytes[4..8].copy_from_slice(&index_kind.to_le_bytes());
    let entry = decode_named_property_entry(&bytes);
    assert_eq!(entry.name_id_or_offset, 0x10);
    assert_eq!(entry.guid_index, 3);
    assert!(entry.is_string);
    assert_eq!(entry.property_index, 5);
}

#[test]
fn oxmsg_named_property_set_sentinels_and_well_known_lookup() {
    let map = NamedPropertyMap {
        guid_stream: PSETID_COMMON.to_vec(),
        entry_stream: Vec::new(),
        string_stream: Vec::new(),
    };
    assert!(matches!(map.resolve_set(1), NamedPropertySet::PsMapi));
    assert!(matches!(
        map.resolve_set(2),
        NamedPropertySet::PsPublicStrings
    ));
    assert!(matches!(
        map.resolve_set(3),
        NamedPropertySet::WellKnown("PSETID_Common")
    ));
    assert!(matches!(map.resolve_set(4), NamedPropertySet::OutOfRange));
}

#[test]
fn oxmsg_named_property_map_lookup_indexes_from_0x8000() {
    // Two 8-byte entries; the second is at byte offset 8.
    let mut entry_stream = vec![0u8; 16];
    entry_stream[8..12].copy_from_slice(&0x2Au32.to_le_bytes());
    let map = NamedPropertyMap {
        guid_stream: Vec::new(),
        entry_stream,
        string_stream: Vec::new(),
    };
    let entry = map.lookup(0x8001).expect("entry at index 1");
    assert_eq!(entry.name_id_or_offset, 0x2A);
    assert!(map.lookup(0x7FFF).is_none()); // below 0x8000
}

#[test]
fn oxmsg_decode_fixed_value_covers_every_fixed_base_type() {
    fn tail_from(bytes: &[u8]) -> [u8; 8] {
        let mut t = [0u8; 8];
        t[..bytes.len()].copy_from_slice(bytes);
        t
    }

    assert_eq!(
        decode_fixed_value(0x0002, &tail_from(&(-5i16).to_le_bytes())),
        Some(DecodedFixedValue::Short(-5))
    );
    assert_eq!(
        decode_fixed_value(0x0003, &tail_from(&42i32.to_le_bytes())),
        Some(DecodedFixedValue::Long(42))
    );
    assert_eq!(
        decode_fixed_value(0x0004, &tail_from(&1.5f32.to_le_bytes())),
        Some(DecodedFixedValue::Float(1.5))
    );
    assert_eq!(
        decode_fixed_value(0x0005, &2.5f64.to_le_bytes()),
        Some(DecodedFixedValue::Double(2.5))
    );
    assert_eq!(
        decode_fixed_value(0x0006, &12345i64.to_le_bytes()),
        Some(DecodedFixedValue::Currency(12345))
    );
    assert_eq!(
        decode_fixed_value(0x0007, &3.75f64.to_le_bytes()),
        Some(DecodedFixedValue::AppTime(3.75))
    );
    assert_eq!(
        decode_fixed_value(0x000A, &tail_from(&99u32.to_le_bytes())),
        Some(DecodedFixedValue::Error(99))
    );
    assert_eq!(
        decode_fixed_value(0x000B, &tail_from(&1u16.to_le_bytes())),
        Some(DecodedFixedValue::Boolean(true))
    );
    assert_eq!(
        decode_fixed_value(0x0014, &123456789i64.to_le_bytes()),
        Some(DecodedFixedValue::I8(123456789))
    );
    assert_eq!(
        decode_fixed_value(0x0040, &99u64.to_le_bytes()),
        Some(DecodedFixedValue::SysTime(99))
    );
    assert_eq!(decode_fixed_value(0x001F, &[0u8; 8]), None); // PT_UNICODE isn't fixed
}

#[test]
fn oxmsg_decode_fixed_value_boolean_treats_any_nonzero_low_byte_as_true() {
    // is_valid_boolean_encoding flags anything other than exactly 0/1
    // as an anomaly, but decode_fixed_value still needs a defined
    // answer for a malformed encoding rather than panicking.
    let mut tail = [0u8; 8];
    tail[0] = 2;
    assert_eq!(
        decode_fixed_value(0x000B, &tail),
        Some(DecodedFixedValue::Boolean(true))
    );
}

#[test]
fn oxmsg_non_finite_float_or_double_is_detected() {
    match decode_fixed_value(0x0005, &f64::NAN.to_le_bytes()) {
        Some(DecodedFixedValue::Double(d)) => assert!(!d.is_finite()),
        _ => panic!("expected Double"),
    }
    let mut inf_tail = [0u8; 8];
    inf_tail[..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    match decode_fixed_value(0x0004, &inf_tail) {
        Some(DecodedFixedValue::Float(f)) => assert!(!f.is_finite()),
        _ => panic!("expected Float"),
    }
}

#[test]
fn oxmsg_decode_unicode_value_handles_valid_and_invalid_utf16() {
    // "Hi" in UTF-16LE.
    let hi: Vec<u8> = vec![0x48, 0x00, 0x69, 0x00];
    assert_eq!(decode_unicode_value(&hi).unwrap(), "Hi");

    // An unpaired low surrogate (0xDC00) is not valid UTF-16.
    let bad: Vec<u8> = vec![0x00, 0xDC];
    assert!(decode_unicode_value(&bad).is_err());
}

#[test]
fn oxmsg_decode_string8_cp1252_maps_latin1_and_flags_undefined_bytes() {
    // 'A' (ASCII), 0xE9 (Latin-1 'e-acute'), 0x93 (left double quote
    // U+201C), 0x81 (undefined in Windows-1252).
    let bytes = [b'A', 0xE9, 0x93, 0x81];
    let (decoded, undefined_count) = decode_string8_cp1252(&bytes);
    let mut chars = decoded.chars();
    assert_eq!(chars.next(), Some('A'));
    assert_eq!(chars.next(), Some('\u{00E9}'));
    assert_eq!(chars.next(), Some('\u{201C}'));
    assert_eq!(chars.next(), Some('\u{FFFD}'));
    assert_eq!(undefined_count, 1);
}

#[test]
fn oxmsg_cp1252_is_latin1_outside_the_defined_upper_range() {
    assert_eq!(cp1252_to_char(b'Z'), Some('Z'));
    assert_eq!(cp1252_to_char(0xE9), Some('\u{00E9}')); // e-acute
    assert_eq!(cp1252_to_char(0x80), Some('\u{20AC}')); // euro sign
    assert_eq!(cp1252_to_char(0x81), None); // genuinely undefined
}

#[test]
fn oxmsg_decode_named_property_string_reads_length_prefixed_utf16() {
    // "Hi" (4-byte length prefix = 4 bytes of UTF-16, then the bytes).
    let mut stream = vec![0u8; 4];
    stream[0..4].copy_from_slice(&4u32.to_le_bytes());
    stream.extend_from_slice(&[0x48, 0x00, 0x69, 0x00]);
    assert_eq!(
        decode_named_property_string(&stream, 0).as_deref(),
        Some("Hi")
    );
}

#[test]
fn oxmsg_decode_named_property_string_rejects_out_of_bounds_length() {
    let mut stream = vec![0u8; 4];
    // Claims 100 bytes follow; the stream doesn't have them.
    stream[0..4].copy_from_slice(&100u32.to_le_bytes());
    assert!(decode_named_property_string(&stream, 0).is_none());
    assert!(decode_named_property_string(&stream, 4).is_none()); // offset past the stream
}

#[test]
fn oxmsg_compare_message_class_covers_all_four_outcomes() {
    assert_eq!(
        compare_message_class(Some("IPM.Note"), Some("IPM.Note")),
        MessageClassComparison::BothPresentMatch
    );
    assert_eq!(
        compare_message_class(Some("IPM.Note"), Some("IPM.Task")),
        MessageClassComparison::BothPresentMismatch
    );
    assert_eq!(
        compare_message_class(Some("IPM.Note"), None),
        MessageClassComparison::PresenceMismatch
    );
    assert_eq!(
        compare_message_class(None, Some("IPM.Note")),
        MessageClassComparison::PresenceMismatch
    );
    assert_eq!(
        compare_message_class(None, None),
        MessageClassComparison::BothAbsent
    );
}

#[test]
fn oxmsg_compare_bool_field_covers_all_three_outcomes() {
    assert_eq!(
        compare_bool_field(true, true),
        BoolFieldComparison::BothTrue
    );
    assert_eq!(
        compare_bool_field(false, false),
        BoolFieldComparison::BothFalse
    );
    assert_eq!(
        compare_bool_field(true, false),
        BoolFieldComparison::Mismatch
    );
    assert_eq!(
        compare_bool_field(false, true),
        BoolFieldComparison::Mismatch
    );
}

#[test]
fn oxmsg_compare_count_matches_and_mismatches() {
    assert_eq!(compare_count(3, 3), CountComparison::Match);
    assert_eq!(compare_count(3, 4), CountComparison::Mismatch);
}

// --- PT_STRING8 code page chain (M3 design debt closed) ---------------

#[test]
fn string8_codepage_chain_prefers_message_then_internet_then_fallback() {
    assert_eq!(
        resolve_string8_codepage(Some(932), Some(1251)),
        ResolvedCodepage {
            codepage: 932,
            source: CodepageSource::MessageCodepage
        }
    );
    // Zero ("use the folder's code page"), negative, and absent are all
    // unspecified and fall through.
    assert_eq!(
        resolve_string8_codepage(Some(0), Some(1251)),
        ResolvedCodepage {
            codepage: 1251,
            source: CodepageSource::InternetCodepage
        }
    );
    assert_eq!(
        resolve_string8_codepage(Some(-1), Some(65001)),
        ResolvedCodepage {
            codepage: 65001,
            source: CodepageSource::InternetCodepage
        }
    );
    assert_eq!(
        resolve_string8_codepage(None, None),
        ResolvedCodepage {
            codepage: 1252,
            source: CodepageSource::Fallback
        }
    );
    assert_eq!(
        resolve_string8_codepage(Some(0), Some(0)).source,
        CodepageSource::Fallback
    );
}

#[test]
fn string8_decoding_covers_implemented_codepages() {
    // 1252: 0x80 is the euro sign; 0x81 is one of the five undefined bytes.
    assert_eq!(
        decode_string8_with_codepage(&[0x80, b'a'], 1252),
        String8Decoded::Decoded {
            text: "\u{20AC}a".to_string(),
            replaced: 0
        }
    );
    assert_eq!(
        decode_string8_with_codepage(&[0x81], 1252),
        String8Decoded::Decoded {
            text: "\u{FFFD}".to_string(),
            replaced: 1
        }
    );
    // ISO-8859-1 maps 0x80 to U+0080 (unlike 1252).
    assert_eq!(
        decode_string8_with_codepage(&[0x80], 28591),
        String8Decoded::Decoded {
            text: "\u{0080}".to_string(),
            replaced: 0
        }
    );
    // US-ASCII: a high byte is a replaced byte, not a silent guess.
    assert_eq!(
        decode_string8_with_codepage(&[b'A', 0x80], 20127),
        String8Decoded::Decoded {
            text: "A\u{FFFD}".to_string(),
            replaced: 1
        }
    );
    // UTF-8: valid input is exact; an invalid byte is counted.
    assert_eq!(
        decode_string8_with_codepage("caf\u{E9}".as_bytes(), 65001),
        String8Decoded::Decoded {
            text: "caf\u{E9}".to_string(),
            replaced: 0
        }
    );
    assert!(matches!(
        decode_string8_with_codepage(&[b'a', 0xFF], 65001),
        String8Decoded::Decoded { replaced: 1, .. }
    ));
}

#[test]
fn string8_decoding_reports_unsupported_codepages_instead_of_guessing() {
    // All-ASCII under a known ASCII superset: exact, flagged.
    assert_eq!(
        decode_string8_with_codepage(b"IPM.Note", 932),
        String8Decoded::AsciiUnderUnsupportedCodepage {
            text: "IPM.Note".to_string()
        }
    );
    // Non-ASCII under an unimplemented code page: no text, reported.
    assert_eq!(
        decode_string8_with_codepage(&[0x82, 0xA0], 932),
        String8Decoded::UnsupportedCodepage
    );
    // All-ASCII under a code page NOT known to be an ASCII superset
    // (e.g. an EBCDIC page) is still unsupported.
    assert_eq!(
        decode_string8_with_codepage(b"IPM.Note", 37),
        String8Decoded::UnsupportedCodepage
    );
}

// --- Synthetic ANSI (.msg with PT_STRING8) fixtures ---------------------
//
// The corpus has no PT_STRING8 property, so these build minimal CFB
// containers in a temp file: a message-level properties stream
// (optionally carrying codepage properties) plus a PT_STRING8
// PidTagMessageClass value stream. UNVERIFIED until run on Windows.

fn write_synthetic_ansi_msg(
    tag: &str,
    message_codepage: Option<i32>,
    internet_codepage: Option<i32>,
    class_bytes: &[u8],
) -> PathBuf {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("tsp-synthetic-{}-{tag}.msg", std::process::id()));
    let mut props = vec![0u8; 32]; // message-scope header
    for (property_id, value) in [
        (PROP_MESSAGE_CODEPAGE, message_codepage),
        (PROP_INTERNET_CODEPAGE, internet_codepage),
    ] {
        if let Some(value) = value {
            props.extend_from_slice(&0x0003u16.to_le_bytes()); // PT_LONG
            props.extend_from_slice(&property_id.to_le_bytes());
            props.extend_from_slice(&0x0000_0006u32.to_le_bytes()); // flags
            props.extend_from_slice(&value.to_le_bytes());
            props.extend_from_slice(&[0u8; 4]);
        }
    }
    let mut comp = cfb::create(&path).expect("create synthetic CFB");
    let mut properties = comp
        .create_stream("/__properties_version1.0")
        .expect("create properties stream");
    properties.write_all(&props).expect("write properties");
    properties.flush().expect("flush properties");
    drop(properties);
    let mut class = comp
        .create_stream("/__substg1.0_001A001E")
        .expect("create class stream");
    class.write_all(class_bytes).expect("write class");
    class.flush().expect("flush class");
    drop(class);
    drop(comp);
    path
}

#[test]
fn synthetic_ansi_msg_class_reads_through_the_codepage_chain() {
    // No codepage properties at all: falls back to Windows-1252.
    let path = write_synthetic_ansi_msg("fallback", None, None, b"IPM.Note");
    let mut comp = cfb::open(&path).expect("open synthetic CFB");
    let resolved = extract_string8_codepage(&mut comp, Path::new("/"), 32);
    assert_eq!(resolved.source, CodepageSource::Fallback);
    assert_eq!(
        extract_message_class(&mut comp).as_deref(),
        Some("IPM.Note")
    );
    drop(comp);
    let _ = std::fs::remove_file(&path);

    // Internet code page only.
    let path = write_synthetic_ansi_msg("internet", None, Some(65001), b"IPM.Note");
    let mut comp = cfb::open(&path).expect("open synthetic CFB");
    let resolved = extract_string8_codepage(&mut comp, Path::new("/"), 32);
    assert_eq!(
        resolved,
        ResolvedCodepage {
            codepage: 65001,
            source: CodepageSource::InternetCodepage
        }
    );
    assert_eq!(
        extract_message_class(&mut comp).as_deref(),
        Some("IPM.Note")
    );
    drop(comp);
    let _ = std::fs::remove_file(&path);

    // Message code page wins over the Internet code page, and an
    // all-ASCII class under a known ASCII superset still reads.
    let path = write_synthetic_ansi_msg("message", Some(932), Some(1251), b"IPM.Note");
    let mut comp = cfb::open(&path).expect("open synthetic CFB");
    let resolved = extract_string8_codepage(&mut comp, Path::new("/"), 32);
    assert_eq!(
        resolved,
        ResolvedCodepage {
            codepage: 932,
            source: CodepageSource::MessageCodepage
        }
    );
    assert_eq!(
        extract_message_class(&mut comp).as_deref(),
        Some("IPM.Note")
    );
    drop(comp);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn synthetic_ansi_msg_with_unsupported_codepage_and_high_bytes_reports_absent() {
    let path = write_synthetic_ansi_msg("unsupported", Some(932), None, &[0x82, 0xA0]);
    let mut comp = cfb::open(&path).expect("open synthetic CFB");
    assert_eq!(extract_message_class(&mut comp), None);
    drop(comp);
    let _ = std::fs::remove_file(&path);
}

// --- Structural gates folded into --verify -----------------------------

#[test]
fn structural_gates_are_all_zero_for_empty_totals() {
    let totals = OxmsgTotals::default();
    let gates = structural_gate_values(&totals);
    assert!(!gates.is_empty());
    assert_eq!(count_structural_gate_violations(&gates), 0);
}

#[test]
fn structural_gates_flag_each_nonzero_gate() {
    let mut totals = OxmsgTotals {
        unrecognized_entries_total: 1,
        variable_string8_unsupported_codepage_total: 2,
        ..Default::default()
    };
    // One embedded-message Reserved sentinel with no message-shaped
    // storage to account for it: a gap of +1.
    totals.attach_data_object_reserved_counts.insert(0x01, 1);
    let gates = structural_gate_values(&totals);
    assert_eq!(count_structural_gate_violations(&gates), 4);
    // total_entries (0) - recognized (0) - opaque (0) - unrecognized (1)
    // is also a negative accounting gap, hence 4, not 3.
}

// --- Fixture-gated both-paths regression gate --------------------------
//
// Runs the same comparison `tsp --verify` prints, over a real `.msg`
// corpus that is deliberately NOT committed (no personal mail data in
// this repository). Skipped, with a note, when TSP_FIXTURE_DIR is
// unset, so plain `cargo test` stays hermetic. Failure messages carry
// counts only, never content.
//
// PowerShell:
//   $env:TSP_FIXTURE_DIR = "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs"
//   cargo test fixture_corpus_verify_is_clean -- --nocapture

/// Files (of the 29-file corpus) whose decompressed RTF length is
/// EXPECTED to differ from msg_parser's, because msg_parser's LZFu
/// preset dictionary diverges from MS-OXRTFCP's (docs/verification/
/// m3-results.md). Raise only with a documented, spec-checked reason.
const KNOWN_RTF_DICTIONARY_DIVERGENCE_FILES: u64 = 1;

#[test]
fn fixture_corpus_verify_is_clean() {
    let Some(dir) = std::env::var_os("TSP_FIXTURE_DIR") else {
        eprintln!("TSP_FIXTURE_DIR not set; skipping fixture-corpus regression gate");
        return;
    };
    let InputKind::Msg { files, .. } =
        classify_input(Path::new(&dir)).expect("TSP_FIXTURE_DIR must classify")
    else {
        panic!("TSP_FIXTURE_DIR must be a directory of .msg files");
    };
    let totals = collect_msg_verify_totals(&files);

    assert_eq!(totals.open_errors_msg_parser, 0, "msg_parser open errors");
    assert_eq!(totals.open_errors_custom, 0, "custom open errors");
    assert_eq!(totals.message_class_both_present_mismatch, 0);
    assert_eq!(totals.message_class_presence_mismatch, 0);
    for (name, tally) in [
        ("body_plain", &totals.body_plain),
        ("body_html_native", &totals.body_html_native),
        ("body_html_via_rtf", &totals.body_html_via_rtf),
        ("body_rtf", &totals.body_rtf),
    ] {
        assert_eq!(tally.mismatch, 0, "{name} mismatches");
    }
    for (name, tally) in [
        ("recipients_to", &totals.recipients_to),
        ("recipients_cc", &totals.recipients_cc),
        ("recipients_bcc", &totals.recipients_bcc),
        ("attachments_total", &totals.attachments_total),
        ("attachments_by_value", &totals.attachments_by_value),
        (
            "attachments_embedded_message",
            &totals.attachments_embedded_message,
        ),
        ("attachments_ole", &totals.attachments_ole),
        ("attachments_other", &totals.attachments_other),
        (
            "attachments_with_content_id",
            &totals.attachments_with_content_id,
        ),
    ] {
        assert_eq!(tally.mismatched, 0, "{name} mismatches");
    }
    assert_eq!(totals.recipient_other_type_total, 0);
    assert_eq!(totals.recipient_unresolved_total, 0);
    assert_eq!(totals.attachment_unresolved_total, 0);
    assert_eq!(totals.attachment_data_stream_missing_total, 0);
    assert_eq!(totals.embedded_message_class_unreadable_total, 0);
    assert!(
        totals.rtf_decompressed_bytes.mismatched <= KNOWN_RTF_DICTIONARY_DIVERGENCE_FILES,
        "rtf length mismatches exceed the documented dictionary divergence"
    );
    let gates = structural_gate_values(&totals.structural);
    let failing: Vec<&str> = gates
        .iter()
        .filter(|(_, value)| *value != 0)
        .map(|(name, _)| *name)
        .collect();
    assert!(failing.is_empty(), "structural gates nonzero: {failing:?}");
}

// --- M3g: transitional flags retired ------------------------------------

#[test]
fn retired_flags_are_rejected_and_verify_is_accepted() {
    assert!(Args::try_parse_from(["tsp", "--oxmsg", "x.msg"]).is_err());
    assert!(Args::try_parse_from(["tsp", "--extract", "x.msg"]).is_err());
    let args = Args::try_parse_from(["tsp", "--verify", "x.msg"]).expect("--verify parses");
    assert!(args.verify);
    let args = Args::try_parse_from(["tsp", "x.msg"]).expect("bare input parses");
    assert!(!args.verify);
}

#[test]
fn structural_breakdown_prints_for_empty_totals_without_panicking() {
    // The breakdown is only printed when a gate fires, so it never
    // runs on a clean corpus; this keeps it exercised regardless.
    print_structural_breakdown(&OxmsgTotals::default());
}
