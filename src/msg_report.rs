//! MSG report: the shared report shape (`MsgTotals`, `print_msg_report`) populated by the custom
//! MS-OXMSG extraction path.

use std::collections::BTreeMap;

use crate::shared::{BodyCounters, CountStats, ZeroByteStats};

// =============================================================================
// MSG diagnostic (msg_parser adapter)
// =============================================================================

/// PidTagAttachMethod values as exposed by `msg_parser`'s `attach_method`
/// field. Same MAPI vocabulary as the PST side (MS-OXCMSG), but msg_parser
/// does not currently expose the by-reference variants (2/3/4) separately,
/// so they fall into `attachments_method_other` here if ever encountered.
pub(crate) const MSG_ATTACH_METHOD_BY_VALUE: u32 = 1;
pub(crate) const MSG_ATTACH_METHOD_EMBEDDED_MESSAGE: u32 = 5;
pub(crate) const MSG_ATTACH_METHOD_OLE: u32 = 6;

/// PT_ERROR (0x000A): per MS-OXMSG, a property that is *not set* on an
/// object is still represented in its `__properties_version1.0` entry
/// array as an entry of this type, carrying PidTagNotFound. Presence
/// checks that match on property ID alone would count these
/// placeholders; they must be type-gated out.
pub(crate) const PROP_TYPE_ERROR: u16 = 0x000A;
/// PT_UNSPECIFIED (0x0000): excluded from presence checks for the same
/// reason as [`PROP_TYPE_ERROR`].
pub(crate) const PROP_TYPE_UNSPECIFIED: u16 = 0x0000;

/// Prints the MSG inventory report, separated from the
/// scan loop (SLAP). Keys are unchanged from previous versions.
pub(crate) fn print_msg_report(totals: &MsgTotals) {
    println!("open_errors={}", totals.open_errors);

    println!("message_class_missing={}", totals.message_class_missing);
    for (class, count) in &totals.message_classes {
        println!("message_class class={class} count={count}");
    }

    println!("bodies_plain={}", totals.bodies.plain);
    println!("bodies_html={}", totals.bodies.html);
    println!("bodies_html_native={}", totals.bodies.html_native);
    println!("bodies_html_via_rtf={}", totals.bodies.html_via_rtf);
    println!("bodies_rtf={}", totals.bodies.rtf);
    println!(
        "rtf_decompression_errors={}",
        totals.bodies.rtf_decompression_errors
    );
    println!(
        "rtf_decompressed_bytes_total={}",
        totals.bodies.rtf_decompressed_bytes_total
    );

    println!("messages_with_recipients={}", totals.recipients.with_any);
    println!("recipients_to={}", totals.recipients_to);
    println!("recipients_cc={}", totals.recipients_cc);
    println!("recipients_bcc={}", totals.recipients_bcc);
    println!("max_recipients_on_a_message={}", totals.recipients.max);

    println!("messages_with_attachments={}", totals.attachments.with_any);
    println!("total_attachments={}", totals.attachments.total);
    println!("max_attachments_on_a_message={}", totals.attachments.max);
    println!(
        "attachments_zero_byte={}",
        totals.zero_byte_attachments.by_value
    );
    println!(
        "attachments_zero_size_other_method={}",
        totals.zero_byte_attachments.other_method
    );
    println!(
        "attachments_with_content_id={}",
        totals.attachments_with_content_id
    );
    println!(
        "attachments_method_by_value={}",
        totals.attachments_method_by_value
    );
    println!(
        "attachments_method_embedded_message={}",
        totals.attachments_method_embedded_message
    );
    println!("attachments_method_ole={}", totals.attachments_method_ole);
    println!(
        "attachments_method_other={}",
        totals.attachments_method_other
    );

    // The actual test of msg_parser's headline capability: does opening an
    // embedded-message attachment as a nested message really work against a
    // real file, not just per the crate's documentation. One level deep
    // only -- deeper recursion is explicitly deferred, not attempted here.
    println!(
        "embedded_messages_opened={}",
        totals.embedded_messages_opened
    );
    println!(
        "embedded_message_open_errors={}",
        totals.embedded_message_open_errors
    );
    for (class, count) in &totals.embedded_message_classes {
        println!("embedded_message_class class={class} count={count}");
    }
}

/// Aggregated MSG inventory. Body availability,
/// recipient/attachment presence/total/max, and zero-size bookkeeping go
/// through the shared counter types; only the msg_parser vocabulary
/// buckets remain direct fields here.
#[derive(Default)]
pub(crate) struct MsgTotals {
    pub(crate) open_errors: u64,

    pub(crate) message_classes: BTreeMap<String, u64>,
    pub(crate) message_class_missing: u64,

    pub(crate) bodies: BodyCounters,

    pub(crate) recipients_to: u64,
    pub(crate) recipients_cc: u64,
    pub(crate) recipients_bcc: u64,
    pub(crate) recipients: CountStats,

    pub(crate) attachments: CountStats,
    pub(crate) zero_byte_attachments: ZeroByteStats,
    pub(crate) attachments_with_content_id: u64,
    pub(crate) attachments_method_by_value: u64,
    pub(crate) attachments_method_embedded_message: u64,
    pub(crate) attachments_method_ole: u64,
    pub(crate) attachments_method_other: u64,

    pub(crate) embedded_messages_opened: u64,
    pub(crate) embedded_message_open_errors: u64,
    pub(crate) embedded_message_classes: BTreeMap<String, u64>,
}

pub(crate) fn record_msg_class(totals: &mut MsgTotals, class: &str) {
    if class.is_empty() {
        totals.message_class_missing += 1;
    } else {
        *totals.message_classes.entry(class.to_string()).or_insert(0) += 1;
    }
}

/// Records recipient counts by type. Presence/total/max bookkeeping goes
/// through the shared [`CountStats`]; only the per-type split is
/// msg_parser-specific.
///
/// Structural gap, not a bug: `msg_parser` exposes only `to`/`cc`/`bcc` on
/// `Outlook`, with no equivalent of the PST side's `PidTagRecipientType`
/// "ORIG" bucket (a recipient recorded as the sender, MS-OXOMSG value 0).
/// If a `.msg` file ever had an ORIG-classified recipient, there is
/// currently no way to detect it through this crate's public API -- it
/// would simply not be counted anywhere. This asymmetry with the PST-side
/// diagnostic is accepted as a known limitation: ORIG recipients are a
/// rare edge case and no fixture evidence has shown one to test against.
pub(crate) fn record_msg_recipients(totals: &mut MsgTotals, to: u64, cc: u64, bcc: u64) {
    totals.recipients_to += to;
    totals.recipients_cc += cc;
    totals.recipients_bcc += bcc;
    totals.recipients.record(to + cc + bcc);
}

pub(crate) fn record_embedded_message_class(totals: &mut MsgTotals, class: &str) {
    if !class.is_empty() {
        *totals
            .embedded_message_classes
            .entry(class.to_string())
            .or_insert(0) += 1;
    }
}
