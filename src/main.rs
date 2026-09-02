use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use outlook_pst::{
    ltp::{
        prop_context::PropertyValue,
        table_context::{TableContext, TableContextInfo, TableRowColumnValue},
    },
    messaging::{folder::Folder as PstFolder, message::Message as PstMessage, store::Store},
    ndb::node_id::NodeId,
};

/// MS-OXPROPS property identifiers used only to check *presence* or read a
/// small, bounded integer/enum value -- never to read or print free-form
/// content (names, addresses, filenames).
const PROP_BODY: u16 = 0x1000; // PidTagBody (plain text body)
const PROP_BODY_HTML: u16 = 0x1013; // PidTagBodyHtml
const PROP_RTF_COMPRESSED: u16 = 0x1009; // PidTagRtfCompressed
const PROP_RECIPIENT_TYPE: u16 = 0x0C15; // PidTagRecipientType
const PROP_ATTACH_SIZE: u16 = 0x0E20; // PidTagAttachSize
const PROP_ATTACH_METHOD: u16 = 0x3705; // PidTagAttachMethod
const PROP_ATTACH_CONTENT_ID: u16 = 0x3712; // PidTagAttachContentId

/// PidTagRecipientType values (MS-OXOMSG).
const RECIPIENT_TYPE_ORIG: i32 = 0;
const RECIPIENT_TYPE_TO: i32 = 1;
const RECIPIENT_TYPE_CC: i32 = 2;
const RECIPIENT_TYPE_BCC: i32 = 3;

/// PidTagAttachMethod values (MS-OXCMSG).
const ATTACH_METHOD_NONE: i32 = 0;
const ATTACH_METHOD_BY_VALUE: i32 = 1;
const ATTACH_METHOD_BY_REFERENCE: i32 = 2;
const ATTACH_METHOD_BY_REFERENCE_RESOLVE: i32 = 3;
const ATTACH_METHOD_BY_REFERENCE_ONLY: i32 = 4;
const ATTACH_METHOD_EMBEDDED_MESSAGE: i32 = 5;
const ATTACH_METHOD_OLE: i32 = 6;

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe PST inventory for the teaspoon Outlook message miner"
)]
struct Args {
    /// PST file to inspect.
    pst: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let store = outlook_pst::open_store(&args.pst).context("failed to open PST")?;

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

    let mut totals = Totals::default();
    walk_folder(store.as_ref(), ipm.as_ref(), &mut totals)?;

    println!("inventory=privacy_safe");
    println!("ipm_subtree=opened");
    println!("folders={}", totals.folders);
    println!("messages={}", totals.messages);
    println!("message_open_errors={}", totals.message_open_errors);
    println!("folder_open_errors={}", totals.folder_open_errors);
    println!("property_values={}", totals.property_values);

    // --- P4a: message class / body-type availability -----------------------
    println!(
        "message_class_read_errors={}",
        totals.message_class_read_errors
    );
    for (class, count) in &totals.message_classes {
        println!("message_class class={class} count={count}");
    }

    println!("bodies_plain={}", totals.bodies_plain);
    println!("bodies_html={}", totals.bodies_html);
    println!("bodies_rtf={}", totals.bodies_rtf);

    // --- P4a: recipient / attachment aggregate counts -----------------------
    println!(
        "messages_with_recipients={}",
        totals.messages_with_recipients
    );
    println!("total_recipients={}", totals.total_recipients);
    println!("max_recipients_on_a_message={}", totals.max_recipients);

    println!(
        "messages_with_attachments={}",
        totals.messages_with_attachments
    );
    println!("total_attachments={}", totals.total_attachments);
    println!("max_attachments_on_a_message={}", totals.max_attachments);

    // --- P4b: recipient type breakdown --------------------------------------
    println!(
        "recipient_row_read_errors={}",
        totals.recipient_row_read_errors
    );
    println!("recipients_orig={}", totals.recipients_orig);
    println!("recipients_to={}", totals.recipients_to);
    println!("recipients_cc={}", totals.recipients_cc);
    println!("recipients_bcc={}", totals.recipients_bcc);
    println!(
        "recipients_type_other={}",
        totals.recipients_type_other
    );
    println!(
        "recipients_type_unknown={}",
        totals.recipients_type_unknown
    );

    // --- P4b: attachment classification -------------------------------------
    println!(
        "attachment_row_read_errors={}",
        totals.attachment_row_read_errors
    );
    println!("attachments_zero_byte={}", totals.attachments_zero_byte);
    println!(
        "attachments_with_content_id={}",
        totals.attachments_with_content_id
    );
    println!(
        "attachments_method_none={}",
        totals.attachments_method_none
    );
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

    Ok(())
}

#[derive(Default)]
struct Totals {
    folders: u64,
    messages: u64,
    message_open_errors: u64,
    folder_open_errors: u64,
    property_values: u64,

    // P4a.
    message_classes: BTreeMap<String, u64>,
    message_class_read_errors: u64,

    bodies_plain: u64,
    bodies_html: u64,
    bodies_rtf: u64,

    messages_with_recipients: u64,
    total_recipients: u64,
    max_recipients: u64,

    messages_with_attachments: u64,
    total_attachments: u64,
    max_attachments: u64,

    // P4b: recipient type breakdown.
    recipient_row_read_errors: u64,
    recipients_orig: u64,
    recipients_to: u64,
    recipients_cc: u64,
    recipients_bcc: u64,
    recipients_type_other: u64,
    recipients_type_unknown: u64,

    // P4b: attachment classification.
    attachment_row_read_errors: u64,
    attachments_zero_byte: u64,
    attachments_with_content_id: u64,
    attachments_method_none: u64,
    attachments_method_by_value: u64,
    attachments_method_by_reference: u64,
    attachments_method_by_reference_resolve: u64,
    attachments_method_by_reference_only: u64,
    attachments_method_embedded_message: u64,
    attachments_method_ole: u64,
    attachments_method_other: u64,
    attachments_method_unknown: u64,
}

fn walk_folder(store: &dyn Store, folder: &dyn PstFolder, totals: &mut Totals) -> Result<()> {
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

fn inspect_message(message: &dyn PstMessage, totals: &mut Totals) {
    let properties = message.properties();
    totals.property_values += properties.iter().count() as u64;

    record_message_class(totals, properties.message_class());

    record_body_flags(
        totals,
        properties.get(PROP_BODY).is_some(),
        properties.get(PROP_BODY_HTML).is_some(),
        properties.get(PROP_RTF_COMPRESSED).is_some(),
    );

    inspect_recipients(message, totals);
    inspect_attachments(message, totals);
}

/// Records a message-class observation. `class` is `Err` when the message
/// has no readable `PidTagMessageClass` property; that is counted separately
/// rather than silently dropped, consistent with the project's no-silent-loss
/// principle (ADR: loss-aware-normalized-representation).
fn record_message_class(totals: &mut Totals, class: std::io::Result<String>) {
    match class {
        Ok(class) => *totals.message_classes.entry(class).or_insert(0) += 1,
        Err(_) => totals.message_class_read_errors += 1,
    }
}

/// Records body-*availability* only. Never receives or touches actual body
/// content -- callers must pass presence booleans, not the property values.
fn record_body_flags(totals: &mut Totals, has_plain: bool, has_html: bool, has_rtf: bool) {
    if has_plain {
        totals.bodies_plain += 1;
    }
    if has_html {
        totals.bodies_html += 1;
    }
    if has_rtf {
        totals.bodies_rtf += 1;
    }
}

fn record_recipients(totals: &mut Totals, count: u64) {
    if count > 0 {
        totals.messages_with_recipients += 1;
    }
    totals.total_recipients += count;
    totals.max_recipients = totals.max_recipients.max(count);
}

fn record_attachments(totals: &mut Totals, count: u64) {
    if count > 0 {
        totals.messages_with_attachments += 1;
    }
    totals.total_attachments += count;
    totals.max_attachments = totals.max_attachments.max(count);
}

/// Buckets a single recipient row by its `PidTagRecipientType` value.
/// `None` means the property was missing or not a 32-bit integer on that row.
fn record_recipient_type(totals: &mut Totals, recipient_type: Option<i32>) {
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
/// `None` means the property was missing or not a 32-bit integer on that row.
fn record_attachment_method(totals: &mut Totals, method: Option<i32>) {
    match method {
        Some(ATTACH_METHOD_NONE) => totals.attachments_method_none += 1,
        Some(ATTACH_METHOD_BY_VALUE) => totals.attachments_method_by_value += 1,
        Some(ATTACH_METHOD_BY_REFERENCE) => totals.attachments_method_by_reference += 1,
        Some(ATTACH_METHOD_BY_REFERENCE_RESOLVE) => {
            totals.attachments_method_by_reference_resolve += 1
        }
        Some(ATTACH_METHOD_BY_REFERENCE_ONLY) => {
            totals.attachments_method_by_reference_only += 1
        }
        Some(ATTACH_METHOD_EMBEDDED_MESSAGE) => totals.attachments_method_embedded_message += 1,
        Some(ATTACH_METHOD_OLE) => totals.attachments_method_ole += 1,
        Some(_) => totals.attachments_method_other += 1,
        None => totals.attachments_method_unknown += 1,
    }
}

/// Records whether an attachment row's `PidTagAttachSize` was exactly zero.
/// `None` (property missing/unreadable) is not counted as zero-byte.
fn record_attachment_size(totals: &mut Totals, size: Option<i32>) {
    if size == Some(0) {
        totals.attachments_zero_byte += 1;
    }
}

/// Records presence (not content) of `PidTagAttachContentId`, a common but
/// not definitive signal that an attachment is referenced inline (e.g. an
/// inline image) rather than a standalone file attachment.
fn record_attachment_content_id_presence(totals: &mut Totals, has_content_id: bool) {
    if has_content_id {
        totals.attachments_with_content_id += 1;
    }
}

/// Locates the index of a column by MAPI property ID within a table's column
/// descriptors, so its value can be looked up per row via [`read_i32_at`] or
/// a presence check.
fn column_index(context: &TableContextInfo, prop_id: u16) -> Option<usize> {
    context.columns().iter().position(|c| c.prop_id() == prop_id)
}

/// Reads a single row's value at `column_idx` as a 32-bit integer, or `None`
/// if the property is absent on this row, the column doesn't exist, or the
/// value isn't a 32-bit integer. Never returns string/binary content.
fn read_i32_at(
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
fn column_has_value(row_values: &[Option<TableRowColumnValue>], column_idx: usize) -> bool {
    row_values
        .get(column_idx)
        .map(Option::is_some)
        .unwrap_or(false)
}

fn inspect_recipients(message: &dyn PstMessage, totals: &mut Totals) {
    let Some(table_rc) = message.recipient_table() else {
        record_recipients(totals, 0);
        return;
    };
    let table: &dyn TableContext = table_rc;
    let context = table.context();
    let type_idx = column_index(context, PROP_RECIPIENT_TYPE);

    let mut count = 0u64;
    for row in table.rows_matrix() {
        count += 1;
        let Ok(row_values) = row.columns(context) else {
            totals.recipient_row_read_errors += 1;
            continue;
        };

        let recipient_type =
            type_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        record_recipient_type(totals, recipient_type);
    }

    record_recipients(totals, count);
}

fn inspect_attachments(message: &dyn PstMessage, totals: &mut Totals) {
    let Some(table_rc) = message.attachment_table() else {
        record_attachments(totals, 0);
        return;
    };
    let table: &dyn TableContext = table_rc;
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

        let size = size_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        record_attachment_size(totals, size);

        let method = method_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        record_attachment_method(totals, method);

        let has_content_id = content_id_idx
            .map(|idx| column_has_value(&row_values, idx))
            .unwrap_or(false);
        record_attachment_content_id_presence(totals, has_content_id);
    }

    record_attachments(totals, count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_class_is_aggregated_by_name() {
        let mut totals = Totals::default();
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Note.SMIME".to_string()));

        assert_eq!(totals.message_classes.get("IPM.Note"), Some(&2));
        assert_eq!(totals.message_classes.get("IPM.Note.SMIME"), Some(&1));
        assert_eq!(totals.message_class_read_errors, 0);
    }

    #[test]
    fn unreadable_message_class_is_counted_not_dropped() {
        let mut totals = Totals::default();
        record_message_class(
            &mut totals,
            Err(std::io::Error::other("missing PidTagMessageClass")),
        );

        assert_eq!(totals.message_class_read_errors, 1);
        assert!(totals.message_classes.is_empty());
    }

    #[test]
    fn body_flags_are_presence_only() {
        let mut totals = Totals::default();
        record_body_flags(&mut totals, true, true, false);
        record_body_flags(&mut totals, true, false, false);

        assert_eq!(totals.bodies_plain, 2);
        assert_eq!(totals.bodies_html, 1);
        assert_eq!(totals.bodies_rtf, 0);
    }

    #[test]
    fn recipient_counts_track_presence_total_and_max() {
        let mut totals = Totals::default();
        record_recipients(&mut totals, 0);
        record_recipients(&mut totals, 3);
        record_recipients(&mut totals, 1);

        assert_eq!(totals.messages_with_recipients, 2);
        assert_eq!(totals.total_recipients, 4);
        assert_eq!(totals.max_recipients, 3);
    }

    #[test]
    fn attachment_counts_track_presence_total_and_max() {
        let mut totals = Totals::default();
        record_attachments(&mut totals, 0);
        record_attachments(&mut totals, 2);
        record_attachments(&mut totals, 5);

        assert_eq!(totals.messages_with_attachments, 2);
        assert_eq!(totals.total_attachments, 7);
        assert_eq!(totals.max_attachments, 5);
    }

    #[test]
    fn recipient_types_are_bucketed_correctly() {
        let mut totals = Totals::default();
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
        let mut totals = Totals::default();
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
        let mut totals = Totals::default();
        record_attachment_size(&mut totals, Some(0));
        record_attachment_size(&mut totals, Some(1024));
        record_attachment_size(&mut totals, None);

        assert_eq!(totals.attachments_zero_byte, 1);
    }

    #[test]
    fn content_id_presence_is_counted_as_a_boolean_not_a_value() {
        let mut totals = Totals::default();
        record_attachment_content_id_presence(&mut totals, true);
        record_attachment_content_id_presence(&mut totals, false);
        record_attachment_content_id_presence(&mut totals, true);

        assert_eq!(totals.attachments_with_content_id, 2);
    }
}
