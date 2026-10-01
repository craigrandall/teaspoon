//! MSG differential verification (`--verify`): custom extraction versus `msg_parser`, field by
//! field, plus the structural gates.

use std::path::PathBuf;

use anyhow::Result;
use msg_parser::Outlook;

use crate::msg_report::{
    MSG_ATTACH_METHOD_BY_VALUE, MSG_ATTACH_METHOD_EMBEDDED_MESSAGE, MSG_ATTACH_METHOD_OLE,
};
use crate::oxmsg_classify::message_shaped_parent_paths;
use crate::oxmsg_extract::{
    extract_attachment_count, extract_attachment_method_counts, extract_body_flags,
    extract_message_class, extract_recipient_type_counts, open_embedded_message,
};
use crate::oxmsg_structure::{inspect_oxmsg, print_structural_breakdown, OxmsgTotals};
use crate::shared::rtf_bytes_contain_fromhtml;

// =============================================================================
// MSG differential verification (--verify): custom MS-OXMSG extraction vs
// msg_parser, field by field
//
// Prints only match/mismatch counts -- never the differing values -- so
// this mode's output stays exactly as safe to share as every other
// diagnostic here, even though it reads real content internally to make
// the comparison.
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CountComparison {
    Match,
    Mismatch,
}

pub(crate) fn compare_count(msg_parser: u64, custom: u64) -> CountComparison {
    if msg_parser == custom {
        CountComparison::Match
    } else {
        CountComparison::Mismatch
    }
}

#[derive(Default)]
pub(crate) struct CountTally {
    pub(crate) matched: u64,
    pub(crate) mismatched: u64,
}

impl CountTally {
    pub(crate) fn record(&mut self, comparison: CountComparison) {
        match comparison {
            CountComparison::Match => self.matched += 1,
            CountComparison::Mismatch => self.mismatched += 1,
        }
    }
}

pub(crate) fn print_count_tally(name: &str, tally: &CountTally) {
    println!("{name}_match={}", tally.matched);
    println!("{name}_mismatch={}", tally.mismatched);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MessageClassComparison {
    BothPresentMatch,
    BothPresentMismatch,
    /// One path found a message class and the other didn't.
    PresenceMismatch,
    BothAbsent,
}

pub(crate) fn compare_message_class(
    msg_parser: Option<&str>,
    custom: Option<&str>,
) -> MessageClassComparison {
    match (msg_parser, custom) {
        (Some(a), Some(b)) if a == b => MessageClassComparison::BothPresentMatch,
        (Some(_), Some(_)) => MessageClassComparison::BothPresentMismatch,
        (None, None) => MessageClassComparison::BothAbsent,
        _ => MessageClassComparison::PresenceMismatch,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BoolFieldComparison {
    BothTrue,
    BothFalse,
    Mismatch,
}

pub(crate) fn compare_bool_field(msg_parser: bool, custom: bool) -> BoolFieldComparison {
    match (msg_parser, custom) {
        (true, true) => BoolFieldComparison::BothTrue,
        (false, false) => BoolFieldComparison::BothFalse,
        _ => BoolFieldComparison::Mismatch,
    }
}

#[derive(Default)]
pub(crate) struct BoolFieldTally {
    pub(crate) both_true: u64,
    pub(crate) both_false: u64,
    pub(crate) mismatch: u64,
}

impl BoolFieldTally {
    pub(crate) fn record(&mut self, comparison: BoolFieldComparison) {
        match comparison {
            BoolFieldComparison::BothTrue => self.both_true += 1,
            BoolFieldComparison::BothFalse => self.both_false += 1,
            BoolFieldComparison::Mismatch => self.mismatch += 1,
        }
    }
}

pub(crate) fn print_bool_field_tally(name: &str, tally: &BoolFieldTally) {
    println!("{name}_both_true={}", tally.both_true);
    println!("{name}_both_false={}", tally.both_false);
    println!("{name}_mismatch={}", tally.mismatch);
}

#[derive(Default)]
pub(crate) struct MsgVerifyTotals {
    pub(crate) open_errors_msg_parser: u64,
    pub(crate) open_errors_custom: u64,
    pub(crate) message_class_both_present_match: u64,
    pub(crate) message_class_both_present_mismatch: u64,
    pub(crate) message_class_presence_mismatch: u64,
    pub(crate) message_class_both_absent: u64,
    pub(crate) body_plain: BoolFieldTally,
    pub(crate) body_html_native: BoolFieldTally,
    pub(crate) body_html_via_rtf: BoolFieldTally,
    pub(crate) body_rtf: BoolFieldTally,
    pub(crate) msg_parser_rtf_decompression_errors: u64,
    pub(crate) custom_rtf_decompression_errors: u64,
    pub(crate) recipients_to: CountTally,
    pub(crate) recipients_cc: CountTally,
    pub(crate) recipients_bcc: CountTally,
    /// Custom-path-only: `msg_parser` has nothing to compare this against.
    /// This is the direct test of the "37 vs 36" recipient-count
    /// hypothesis.
    pub(crate) recipient_orig_total: u64,
    pub(crate) recipient_other_type_total: u64,
    pub(crate) recipient_unresolved_total: u64,
    pub(crate) attachments_total: CountTally,
    pub(crate) attachments_by_value: CountTally,
    pub(crate) attachments_embedded_message: CountTally,
    pub(crate) attachments_ole: CountTally,
    pub(crate) attachments_other: CountTally,
    pub(crate) attachments_with_content_id: CountTally,
    pub(crate) attachment_unresolved_total: u64,
    /// Custom-path-only: by-value attachments whose PidTagAttachDataBinary
    /// could not be read at all (absent or a CFB read error). Counted on
    /// its own -- an unreadable stream is an anomaly, not an empty file.
    pub(crate) attachment_data_stream_missing_total: u64,
    pub(crate) rtf_decompressed_bytes: CountTally,
    /// Byte deltas (custom minus msg_parser) for the rare case where
    /// decompressed RTF length disagrees -- not content, just a size
    /// difference, kept to confirm the magnitude matches what a narrow
    /// dictionary-region divergence would produce rather than something
    /// larger and less explicable.
    pub(crate) rtf_decompressed_byte_mismatch_deltas: Vec<i64>,
    pub(crate) embedded_message_class_readable_total: u64,
    pub(crate) embedded_message_class_unreadable_total: u64,
    /// The custom path's structural accounting over the same files,
    /// accumulated by `inspect_oxmsg`.
    pub(crate) structural: OxmsgTotals,
}

/// Runs both the custom extraction path and `msg_parser` over the same
/// files and compares their output field by field.
pub(crate) fn run_msg_verify(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    let totals = collect_msg_verify_totals(files);
    println!("inventory=privacy_safe");
    println!("input_kind=msg_verify");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");
    print_msg_verify_report(&totals);
    Ok(())
}

/// Runs both extraction paths over every file and accumulates the
/// comparison tallies plus the custom path's structural accounting,
/// separated from printing (SLAP) so the fixture-gated regression test can
/// assert on the totals directly.
pub(crate) fn collect_msg_verify_totals(files: &[PathBuf]) -> MsgVerifyTotals {
    let mut totals = MsgVerifyTotals::default();

    for file in files {
        let outlook = match Outlook::from_path(file) {
            Ok(outlook) => outlook,
            Err(_) => {
                totals.open_errors_msg_parser += 1;
                continue;
            }
        };
        let mut comp = match cfb::open(file) {
            Ok(comp) => comp,
            Err(_) => {
                totals.open_errors_custom += 1;
                continue;
            }
        };

        let msg_parser_class =
            (!outlook.message_class.is_empty()).then_some(outlook.message_class.as_str());
        let custom_class = extract_message_class(&mut comp);
        match compare_message_class(msg_parser_class, custom_class.as_deref()) {
            MessageClassComparison::BothPresentMatch => {
                totals.message_class_both_present_match += 1;
            }
            MessageClassComparison::BothPresentMismatch => {
                totals.message_class_both_present_mismatch += 1;
            }
            MessageClassComparison::PresenceMismatch => {
                totals.message_class_presence_mismatch += 1;
            }
            MessageClassComparison::BothAbsent => {
                totals.message_class_both_absent += 1;
            }
        }

        let mp_has_plain = !outlook.body.is_empty();
        let mp_has_html_native = !outlook.html.is_empty();
        let mp_has_rtf = !outlook.rtf_compressed.is_empty();
        let mp_has_html_via_rtf = if mp_has_html_native {
            false
        } else if mp_has_rtf {
            match outlook.rtf_decompressed() {
                Some(bytes) => rtf_bytes_contain_fromhtml(&bytes),
                None => {
                    totals.msg_parser_rtf_decompression_errors += 1;
                    false
                }
            }
        } else {
            false
        };

        let custom_body = extract_body_flags(&mut comp);
        if custom_body.decompression_failed {
            totals.custom_rtf_decompression_errors += 1;
        }

        let mp_rtf_decompressed_len = outlook.rtf_decompressed().map(|bytes| bytes.len() as u64);
        let mp_len = mp_rtf_decompressed_len.unwrap_or(0);
        let custom_len = custom_body.decompressed_rtf_len.unwrap_or(0);
        totals
            .rtf_decompressed_bytes
            .record(compare_count(mp_len, custom_len));
        if mp_len != custom_len {
            totals
                .rtf_decompressed_byte_mismatch_deltas
                .push(custom_len as i64 - mp_len as i64);
        }

        totals
            .body_plain
            .record(compare_bool_field(mp_has_plain, custom_body.has_plain));
        totals.body_html_native.record(compare_bool_field(
            mp_has_html_native,
            custom_body.has_html_native,
        ));
        totals.body_html_via_rtf.record(compare_bool_field(
            mp_has_html_via_rtf,
            custom_body.has_html_via_rtf,
        ));
        totals
            .body_rtf
            .record(compare_bool_field(mp_has_rtf, custom_body.has_rtf));

        let recipient_counts = extract_recipient_type_counts(&mut comp);
        totals
            .recipients_to
            .record(compare_count(outlook.to.len() as u64, recipient_counts.to));
        totals
            .recipients_cc
            .record(compare_count(outlook.cc.len() as u64, recipient_counts.cc));
        totals.recipients_bcc.record(compare_count(
            outlook.bcc.len() as u64,
            recipient_counts.bcc,
        ));
        totals.recipient_orig_total += recipient_counts.orig;
        totals.recipient_other_type_total += recipient_counts.other;
        totals.recipient_unresolved_total += recipient_counts.unresolved;

        totals.attachments_total.record(compare_count(
            outlook.attachments.len() as u64,
            extract_attachment_count(&comp),
        ));

        let mut mp_by_value = 0u64;
        let mut mp_embedded_message = 0u64;
        let mut mp_ole = 0u64;
        let mut mp_other = 0u64;
        let mut mp_with_content_id = 0u64;
        for attach in &outlook.attachments {
            match attach.attach_method {
                MSG_ATTACH_METHOD_BY_VALUE => mp_by_value += 1,
                MSG_ATTACH_METHOD_EMBEDDED_MESSAGE => mp_embedded_message += 1,
                MSG_ATTACH_METHOD_OLE => mp_ole += 1,
                _ => mp_other += 1,
            }
            if !attach.content_id.is_empty() {
                mp_with_content_id += 1;
            }
        }

        let attachment_methods = extract_attachment_method_counts(&mut comp);
        totals
            .attachments_by_value
            .record(compare_count(mp_by_value, attachment_methods.by_value));
        totals.attachments_embedded_message.record(compare_count(
            mp_embedded_message,
            attachment_methods.embedded_message,
        ));
        totals
            .attachments_ole
            .record(compare_count(mp_ole, attachment_methods.ole));
        totals
            .attachments_other
            .record(compare_count(mp_other, attachment_methods.other));
        totals.attachments_with_content_id.record(compare_count(
            mp_with_content_id,
            attachment_methods.with_content_id,
        ));
        totals.attachment_unresolved_total += attachment_methods.unresolved;
        totals.attachment_data_stream_missing_total += attachment_methods.zero_data_stream_missing;

        // Per embedded-message attachment, matching the default report; the
        // previous once-per-file gate undercounted any message with more
        // than one embedded-message attachment.
        let message_shaped = message_shaped_parent_paths(&comp);
        for attach_path in &attachment_methods.embedded_paths {
            match open_embedded_message(&mut comp, attach_path, &message_shaped) {
                Some(_) => totals.embedded_message_class_readable_total += 1,
                None => totals.embedded_message_class_unreadable_total += 1,
            }
        }

        // Structural accounting (formerly the separate --oxmsg mode):
        // every CFB entry classified, every properties stream and value
        // stream decoded. Content-free counters only.
        inspect_oxmsg(&mut comp, &mut totals.structural);
    }

    totals
}

/// Prints the `--verify` report, separated from the scan loop (SLAP).
pub(crate) fn print_msg_verify_report(totals: &MsgVerifyTotals) {
    println!("open_errors_msg_parser={}", totals.open_errors_msg_parser);
    println!("open_errors_custom={}", totals.open_errors_custom);
    println!(
        "message_class_both_present_match={}",
        totals.message_class_both_present_match
    );
    println!(
        "message_class_both_present_mismatch={}",
        totals.message_class_both_present_mismatch
    );
    println!(
        "message_class_presence_mismatch={}",
        totals.message_class_presence_mismatch
    );
    println!(
        "message_class_both_absent={}",
        totals.message_class_both_absent
    );
    print_bool_field_tally("body_plain", &totals.body_plain);
    print_bool_field_tally("body_html_native", &totals.body_html_native);
    print_bool_field_tally("body_html_via_rtf", &totals.body_html_via_rtf);
    print_bool_field_tally("body_rtf", &totals.body_rtf);
    println!(
        "msg_parser_rtf_decompression_errors={}",
        totals.msg_parser_rtf_decompression_errors
    );
    println!(
        "custom_rtf_decompression_errors={}",
        totals.custom_rtf_decompression_errors
    );
    print_count_tally("recipients_to", &totals.recipients_to);
    print_count_tally("recipients_cc", &totals.recipients_cc);
    print_count_tally("recipients_bcc", &totals.recipients_bcc);
    println!("recipient_orig_total={}", totals.recipient_orig_total);
    println!(
        "recipient_other_type_total={}",
        totals.recipient_other_type_total
    );
    println!(
        "recipient_unresolved_total={}",
        totals.recipient_unresolved_total
    );
    print_count_tally("attachments_total", &totals.attachments_total);
    print_count_tally("attachments_by_value", &totals.attachments_by_value);
    print_count_tally(
        "attachments_embedded_message",
        &totals.attachments_embedded_message,
    );
    print_count_tally("attachments_ole", &totals.attachments_ole);
    print_count_tally("attachments_other", &totals.attachments_other);
    print_count_tally(
        "attachments_with_content_id",
        &totals.attachments_with_content_id,
    );
    println!(
        "attachment_unresolved_total={}",
        totals.attachment_unresolved_total
    );
    println!(
        "attachment_data_stream_missing_total={}",
        totals.attachment_data_stream_missing_total
    );
    print_count_tally("rtf_decompressed_bytes", &totals.rtf_decompressed_bytes);
    for delta in &totals.rtf_decompressed_byte_mismatch_deltas {
        println!("rtf_decompressed_bytes_mismatch_delta={delta}");
    }
    println!(
        "embedded_message_class_readable_total={}",
        totals.embedded_message_class_readable_total
    );
    println!(
        "embedded_message_class_unreadable_total={}",
        totals.embedded_message_class_unreadable_total
    );

    print_verify_structural_gates(&totals.structural);
}

/// Structural gate counters, each of which MUST read 0 on a clean corpus.
/// Every one has direct corpus evidence of reading 0 (see
/// docs/verification/m3-results.md); the value is signed so an overcount
/// is visible. Names are the output vocabulary the retired `--oxmsg` used.
pub(crate) fn structural_gate_values(totals: &OxmsgTotals) -> Vec<(&'static str, i64)> {
    let reserved_embedded = totals
        .attach_data_object_reserved_counts
        .get(&0x01)
        .copied()
        .unwrap_or(0);
    let reserved_storage = totals
        .attach_data_object_reserved_counts
        .get(&0x04)
        .copied()
        .unwrap_or(0);
    vec![
        (
            "entry_accounting_gap_total",
            totals.entry_accounting_gap_total(),
        ),
        (
            "unrecognized_entries_total",
            totals.unrecognized_entries_total as i64,
        ),
        (
            "properties_stream_too_short_for_header_total",
            totals.properties_stream_too_short_for_header_total as i64,
        ),
        (
            "properties_stream_trailing_bytes_total",
            totals.properties_stream_trailing_bytes_total as i64,
        ),
        (
            "properties_stream_unexpected_scope_total",
            totals.properties_stream_unexpected_scope_total as i64,
        ),
        (
            "properties_stream_read_errors",
            totals.properties_stream_read_errors as i64,
        ),
        (
            "attach_data_object_size_sentinel_mismatches",
            totals.attach_data_object_size_sentinel_mismatches as i64,
        ),
        (
            "attach_data_object_reserved_vs_embedded_object_storage_gap",
            reserved_embedded as i64 - totals.embedded_object_storages_message_shaped_total as i64,
        ),
        (
            "attach_data_object_reserved_vs_custom_object_storage_gap",
            reserved_storage as i64 - totals.embedded_object_storages_custom_total as i64,
        ),
        (
            "fixed_boolean_invalid_encoding_total",
            totals.fixed_boolean_invalid_encoding_total as i64,
        ),
        (
            "fixed_float_non_finite_total",
            totals.fixed_float_non_finite_total as i64,
        ),
        (
            "variable_value_stream_missing_total",
            totals.variable_value_stream_missing_total as i64,
        ),
        (
            "variable_value_size_mismatch_total",
            totals.variable_value_size_mismatch_total as i64,
        ),
        (
            "variable_value_odd_utf16_length_total",
            totals.variable_value_odd_utf16_length_total as i64,
        ),
        (
            "variable_unicode_decode_errors_total",
            totals.variable_unicode_decode_errors_total as i64,
        ),
        (
            "variable_string8_undefined_byte_total",
            totals.variable_string8_undefined_byte_total as i64,
        ),
        (
            "variable_string8_unsupported_codepage_total",
            totals.variable_string8_unsupported_codepage_total as i64,
        ),
        (
            "variable_clsid_wrong_length_total",
            totals.variable_clsid_wrong_length_total as i64,
        ),
        (
            "named_properties_guid_out_of_range_total",
            totals.named_properties_guid_out_of_range_total as i64,
        ),
        (
            "named_properties_string_decode_errors_total",
            totals.named_properties_string_decode_errors_total as i64,
        ),
        (
            "named_properties_index_mismatch_total",
            totals.named_properties_index_mismatch_total as i64,
        ),
    ]
}

pub(crate) fn count_structural_gate_violations(gates: &[(&'static str, i64)]) -> usize {
    gates.iter().filter(|(_, value)| *value != 0).count()
}

/// Prints the structural gates, a single violation count (0 on a clean
/// corpus), and a few informational counters that are reported but not
/// gated. When any gate is nonzero the full structural breakdown follows,
/// so the violation can be triaged from the same run.
pub(crate) fn print_verify_structural_gates(totals: &OxmsgTotals) {
    let gates = structural_gate_values(totals);
    for (name, value) in &gates {
        println!("{name}={value}");
    }
    println!(
        "structural_gate_violations={}",
        count_structural_gate_violations(&gates)
    );
    println!("total_entries={}", totals.total_entries);
    println!(
        "opaque_payload_entries_total={}",
        totals.opaque_payload_entries_total
    );
    println!(
        "named_properties_map_missing_total={}",
        totals.named_properties_map_missing_total
    );
    println!(
        "named_properties_unresolvable_total={}",
        totals.named_properties_unresolvable_total
    );
    println!(
        "variable_string8_ascii_under_unsupported_codepage_total={}",
        totals.variable_string8_ascii_under_unsupported_codepage_total
    );
    println!(
        "string8_codepage_from_message_total={}",
        totals.string8_codepage_from_message_total
    );
    println!(
        "string8_codepage_from_internet_total={}",
        totals.string8_codepage_from_internet_total
    );
    println!(
        "string8_codepage_fallback_total={}",
        totals.string8_codepage_fallback_total
    );
    if count_structural_gate_violations(&gates) > 0 {
        println!("structural_breakdown=follows");
        print_structural_breakdown(totals);
    }
}
