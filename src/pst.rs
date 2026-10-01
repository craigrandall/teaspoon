//! PST diagnostic (M1): walks the IPM subtree via `outlook_pst` and prints a privacy-safe
//! inventory.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::ltp::table_context::{TableContext, TableContextInfo, TableRowColumnValue};
use outlook_pst::messaging::folder::Folder as PstFolder;
use outlook_pst::messaging::message::Message as PstMessage;
use outlook_pst::messaging::store::Store;
use outlook_pst::ndb::node_id::NodeId;

use crate::shared::{
    check_compressed_rtf_bytes, BodyCounters, CountStats, RtfHtmlCheck, ZeroByteStats,
    ATTACH_METHOD_BY_REFERENCE, ATTACH_METHOD_BY_REFERENCE_ONLY,
    ATTACH_METHOD_BY_REFERENCE_RESOLVE, ATTACH_METHOD_BY_VALUE, ATTACH_METHOD_EMBEDDED_MESSAGE,
    ATTACH_METHOD_NONE, ATTACH_METHOD_OLE, PROP_ATTACH_CONTENT_ID, PROP_ATTACH_METHOD,
    PROP_ATTACH_SIZE, PROP_BODY, PROP_BODY_HTML, PROP_RECIPIENT_TYPE, PROP_RTF_COMPRESSED,
    RECIPIENT_TYPE_BCC, RECIPIENT_TYPE_CC, RECIPIENT_TYPE_ORIG, RECIPIENT_TYPE_TO,
};

// =============================================================================
// PST diagnostic (M1)
// =============================================================================

pub(crate) fn run_pst_diagnostic(path: &Path) -> Result<()> {
    let store = outlook_pst::open_store(path).context("failed to open PST")?;

    // Deliberately do not print the input path or PST display name. Both can
    // disclose information about the user or their mailbox.

    let ipm_id = store
        .properties()
        .ipm_sub_tree_entry_id()
        .context("PST has no IPM subtree entry ID")?;

    let ipm = store
        .open_folder(&ipm_id)
        .context("failed to open IPM subtree")?;

    println!("ipm_subtree=ok");

    let mut totals = PstTotals::default();
    walk_folder(store.as_ref(), ipm.as_ref(), &mut totals)?;

    print_pst_report(&totals);

    Ok(())
}

/// Prints the PST inventory report, separated from the walk logic so each
/// reads at one level of abstraction (SLAP). Every key printed here is
/// part of the tool's stable, privacy-safe output vocabulary -- keys are
/// unchanged from previous versions.
pub(crate) fn print_pst_report(totals: &PstTotals) {
    println!("inventory=privacy_safe");
    println!("input_kind=pst");
    println!("ipm_subtree=opened");
    println!("folders={}", totals.folders);
    println!("messages={}", totals.messages);
    println!("message_open_errors={}", totals.message_open_errors);
    println!("folder_open_errors={}", totals.folder_open_errors);
    println!("property_values={}", totals.property_values);

    // --- message class / body-type availability ------------------------------
    println!(
        "message_class_read_errors={}",
        totals.message_class_read_errors
    );
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

    // --- recipient / attachment aggregate counts ----------------------------
    println!("messages_with_recipients={}", totals.recipients.with_any);
    println!("total_recipients={}", totals.recipients.total);
    println!("max_recipients_on_a_message={}", totals.recipients.max);

    println!("messages_with_attachments={}", totals.attachments.with_any);
    println!("total_attachments={}", totals.attachments.total);
    println!("max_attachments_on_a_message={}", totals.attachments.max);

    // --- recipient type breakdown -------------------------------------------
    println!(
        "recipient_row_read_errors={}",
        totals.recipient_row_read_errors
    );
    println!("recipients_orig={}", totals.recipients_orig);
    println!("recipients_to={}", totals.recipients_to);
    println!("recipients_cc={}", totals.recipients_cc);
    println!("recipients_bcc={}", totals.recipients_bcc);
    println!("recipients_type_other={}", totals.recipients_type_other);
    println!("recipients_type_unknown={}", totals.recipients_type_unknown);

    // --- attachment classification --------------------------------------------
    println!(
        "attachment_row_read_errors={}",
        totals.attachment_row_read_errors
    );
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
    println!("attachments_method_none={}", totals.attachments_method_none);
    println!(
        "attachments_method_by_value={}",
        totals.attachments_method_by_value
    );
    println!(
        "attachments_method_by_reference={}",
        totals.attachments_method_by_reference
    );
    println!(
        "attachments_method_by_reference_resolve={}",
        totals.attachments_method_by_reference_resolve
    );
    println!(
        "attachments_method_by_reference_only={}",
        totals.attachments_method_by_reference_only
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
    println!(
        "attachments_method_unknown={}",
        totals.attachments_method_unknown
    );
}

/// Aggregated PST inventory. Body availability, recipient/attachment
/// presence/total/max, and zero-size bookkeeping all go through the
/// shared counter types; only the PST-specific buckets remain direct
/// fields here.
#[derive(Default)]
pub(crate) struct PstTotals {
    pub(crate) folders: u64,
    pub(crate) messages: u64,
    pub(crate) message_open_errors: u64,
    pub(crate) folder_open_errors: u64,
    pub(crate) property_values: u64,

    pub(crate) message_classes: BTreeMap<String, u64>,
    pub(crate) message_class_read_errors: u64,

    pub(crate) bodies: BodyCounters,
    pub(crate) recipients: CountStats,
    pub(crate) attachments: CountStats,

    // Recipient type breakdown.
    pub(crate) recipient_row_read_errors: u64,
    pub(crate) recipients_orig: u64,
    pub(crate) recipients_to: u64,
    pub(crate) recipients_cc: u64,
    pub(crate) recipients_bcc: u64,
    pub(crate) recipients_type_other: u64,
    pub(crate) recipients_type_unknown: u64,

    // Attachment classification.
    pub(crate) attachment_row_read_errors: u64,
    pub(crate) zero_byte_attachments: ZeroByteStats,
    pub(crate) attachments_with_content_id: u64,
    pub(crate) attachments_method_none: u64,
    pub(crate) attachments_method_by_value: u64,
    pub(crate) attachments_method_by_reference: u64,
    pub(crate) attachments_method_by_reference_resolve: u64,
    pub(crate) attachments_method_by_reference_only: u64,
    pub(crate) attachments_method_embedded_message: u64,
    pub(crate) attachments_method_ole: u64,
    pub(crate) attachments_method_other: u64,
    pub(crate) attachments_method_unknown: u64,
}

pub(crate) fn walk_folder(
    store: &dyn Store,
    folder: &dyn PstFolder,
    totals: &mut PstTotals,
) -> Result<()> {
    totals.folders += 1;

    if let Some(contents) = folder.contents_table() {
        for row in contents.rows_matrix() {
            totals.messages += 1;

            let entry_id = match store
                .properties()
                .make_entry_id(NodeId::from(u32::from(row.id())))
            {
                Ok(entry_id) => entry_id,
                Err(_) => {
                    totals.message_open_errors += 1;
                    continue;
                }
            };

            match store.open_message(&entry_id, None) {
                Ok(message) => inspect_message(message.as_ref(), totals),
                Err(_) => totals.message_open_errors += 1,
            }
        }
        // Folder names, message subjects, entry IDs, and other property values
        // are intentionally excluded from diagnostic output.
    }

    if let Some(hierarchy) = folder.hierarchy_table() {
        for row in hierarchy.rows_matrix() {
            let node = NodeId::from(u32::from(row.id()));

            let entry_id = match store.properties().make_entry_id(node) {
                Ok(entry_id) => entry_id,
                Err(_) => {
                    totals.folder_open_errors += 1;
                    continue;
                }
            };

            match store.open_folder(&entry_id) {
                Ok(child) => walk_folder(store, child.as_ref(), totals)?,
                Err(_) => totals.folder_open_errors += 1,
            }
        }
    }

    Ok(())
}

pub(crate) fn inspect_message(message: &dyn PstMessage, totals: &mut PstTotals) {
    let properties = message.properties();
    totals.property_values += properties.iter().count() as u64;

    record_message_class(totals, properties.message_class());

    // HTML detection has two layers, mirroring the MSG side: many real
    // messages have no native PidTagBodyHtml at all -- Outlook instead
    // encapsulates the HTML inside PidTagRtfCompressed per MS-OXRTFEX,
    // detectable via the FROMHTML control word. Presence-only check: tsp
    // never needs the extracted HTML/RTF content itself.
    let has_html_native = properties.get(PROP_BODY_HTML).is_some();
    let has_rtf = properties.get(PROP_RTF_COMPRESSED).is_some();
    let has_html_via_rtf = if has_html_native {
        false
    } else {
        match check_rtf_for_encapsulated_html(properties.get(PROP_RTF_COMPRESSED)) {
            RtfHtmlCheck::Decompressed {
                contains_fromhtml,
                decompressed_bytes,
            } => {
                totals.bodies.note_decompressed_bytes(decompressed_bytes);
                contains_fromhtml
            }
            RtfHtmlCheck::DecompressionFailed => {
                totals.bodies.note_decompression_error();
                false
            }
            RtfHtmlCheck::NoRtfProperty | RtfHtmlCheck::NotBinary => false,
        }
    };

    totals.bodies.record(
        properties.get(PROP_BODY).is_some(),
        has_html_native,
        has_html_via_rtf,
        has_rtf,
    );

    inspect_recipients(message, totals);
    inspect_attachments(message, totals);
}

/// PST-side wrapper around [`check_compressed_rtf_bytes`]: extracts the
/// binary buffer from a `PidTagRtfCompressed` property value, keeping the
/// "no property at all" and "present but not binary" cases distinct from
/// genuine decompression failures. Never exposes RTF/HTML content.
pub(crate) fn check_rtf_for_encapsulated_html(
    rtf_property: Option<&PropertyValue>,
) -> RtfHtmlCheck {
    let Some(value) = rtf_property else {
        return RtfHtmlCheck::NoRtfProperty;
    };
    let PropertyValue::Binary(binary) = value else {
        return RtfHtmlCheck::NotBinary;
    };
    check_compressed_rtf_bytes(binary.buffer())
}

/// Records a message-class observation. `class` is `Err` when the message
/// has no readable `PidTagMessageClass` property; that is counted
/// separately rather than silently dropped, consistent with the project's
/// no-silent-loss principle.
pub(crate) fn record_message_class(totals: &mut PstTotals, class: std::io::Result<String>) {
    match class {
        Ok(class) => *totals.message_classes.entry(class).or_insert(0) += 1,
        Err(_) => totals.message_class_read_errors += 1,
    }
}

/// Buckets a single recipient row by its `PidTagRecipientType` value.
/// `None` means the property was missing or not a 32-bit integer on that
/// row.
pub(crate) fn record_recipient_type(totals: &mut PstTotals, recipient_type: Option<i32>) {
    match recipient_type {
        Some(RECIPIENT_TYPE_ORIG) => totals.recipients_orig += 1,
        Some(RECIPIENT_TYPE_TO) => totals.recipients_to += 1,
        Some(RECIPIENT_TYPE_CC) => totals.recipients_cc += 1,
        Some(RECIPIENT_TYPE_BCC) => totals.recipients_bcc += 1,
        Some(_) => totals.recipients_type_other += 1,
        None => totals.recipients_type_unknown += 1,
    }
}

/// Buckets a single attachment row by its `PidTagAttachMethod` value.
/// `None` means the property was missing or not a 32-bit integer on that
/// row.
pub(crate) fn record_attachment_method(totals: &mut PstTotals, method: Option<i32>) {
    match method {
        Some(ATTACH_METHOD_NONE) => totals.attachments_method_none += 1,
        Some(ATTACH_METHOD_BY_VALUE) => totals.attachments_method_by_value += 1,
        Some(ATTACH_METHOD_BY_REFERENCE) => totals.attachments_method_by_reference += 1,
        Some(ATTACH_METHOD_BY_REFERENCE_RESOLVE) => {
            totals.attachments_method_by_reference_resolve += 1
        }
        Some(ATTACH_METHOD_BY_REFERENCE_ONLY) => totals.attachments_method_by_reference_only += 1,
        Some(ATTACH_METHOD_EMBEDDED_MESSAGE) => totals.attachments_method_embedded_message += 1,
        Some(ATTACH_METHOD_OLE) => totals.attachments_method_ole += 1,
        Some(_) => totals.attachments_method_other += 1,
        None => totals.attachments_method_unknown += 1,
    }
}

/// Records presence (not content) of `PidTagAttachContentId`, a common but
/// not definitive signal that an attachment is referenced inline (e.g. an
/// inline image) rather than a standalone file attachment. Zero-size
/// bookkeeping goes through the shared [`ZeroByteStats`] instead.
pub(crate) fn record_attachment_content_id_presence(totals: &mut PstTotals, has_content_id: bool) {
    if has_content_id {
        totals.attachments_with_content_id += 1;
    }
}

/// Locates the index of a column by MAPI property ID within a table's
/// column descriptors, so its value can be looked up per row via
/// [`read_i32_at`] or a presence check.
pub(crate) fn column_index(context: &TableContextInfo, prop_id: u16) -> Option<usize> {
    context
        .columns()
        .iter()
        .position(|c| c.prop_id() == prop_id)
}

/// Reads a single row's value at `column_idx` as a 32-bit integer, or
/// `None` if the property is absent on this row, the column doesn't exist,
/// or the value isn't a 32-bit integer. Never returns string/binary
/// content.
///
/// Untested assumption: `PidTagRecipientType`, `PidTagAttachMethod`, and
/// `PidTagAttachSize` always arrive as `PropertyValue::Integer32` -- every
/// fixture so far stored them that way. A PST storing one differently would
/// fall through to `None` (bucketed as "unknown" by callers) rather than
/// panicking or miscounting, so the failure mode is safe; the assumption
/// itself has never been falsified because it has never been tested
/// against data that would break it.
pub(crate) fn read_i32_at(
    table: &dyn TableContext,
    context: &TableContextInfo,
    row_values: &[Option<TableRowColumnValue>],
    column_idx: usize,
) -> Option<i32> {
    let descriptor = context.columns().get(column_idx)?;
    let value = row_values.get(column_idx)?.as_ref()?;
    match table.read_column(value, descriptor.prop_type()).ok()? {
        PropertyValue::Integer32(v) => Some(v),
        _ => None,
    }
}

/// Reports only whether a row has *any* value in `column_idx` -- used for
/// PidTagAttachContentId, where presence (not content) is the signal.
pub(crate) fn column_has_value(
    row_values: &[Option<TableRowColumnValue>],
    column_idx: usize,
) -> bool {
    row_values
        .get(column_idx)
        .map(Option::is_some)
        .unwrap_or(false)
}

pub(crate) fn inspect_recipients(message: &dyn PstMessage, totals: &mut PstTotals) {
    let Some(table_rc) = message.recipient_table() else {
        totals.recipients.record(0);
        return;
    };
    let table: &dyn TableContext = table_rc.as_ref();
    let context = table.context();
    let type_idx = column_index(context, PROP_RECIPIENT_TYPE);

    let mut count = 0u64;
    for row in table.rows_matrix() {
        count += 1;
        let Ok(row_values) = row.columns(context) else {
            totals.recipient_row_read_errors += 1;
            continue;
        };

        let recipient_type = type_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        record_recipient_type(totals, recipient_type);
    }

    totals.recipients.record(count);
}

pub(crate) fn inspect_attachments(message: &dyn PstMessage, totals: &mut PstTotals) {
    let Some(table_rc) = message.attachment_table() else {
        totals.attachments.record(0);
        return;
    };
    let table: &dyn TableContext = table_rc.as_ref();
    let context = table.context();
    let size_idx = column_index(context, PROP_ATTACH_SIZE);
    let method_idx = column_index(context, PROP_ATTACH_METHOD);
    let content_id_idx = column_index(context, PROP_ATTACH_CONTENT_ID);

    let mut count = 0u64;
    for row in table.rows_matrix() {
        count += 1;
        let Ok(row_values) = row.columns(context) else {
            totals.attachment_row_read_errors += 1;
            continue;
        };

        let method = method_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));

        let size = size_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        totals
            .zero_byte_attachments
            .record(size == Some(0), method == Some(ATTACH_METHOD_BY_VALUE));

        record_attachment_method(totals, method);

        let has_content_id = content_id_idx
            .map(|idx| column_has_value(&row_values, idx))
            .unwrap_or(false);
        record_attachment_content_id_presence(totals, has_content_id);
    }

    totals.attachments.record(count);
}
