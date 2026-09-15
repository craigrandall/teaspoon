use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use msg_parser::Outlook;
use outlook_pst::{
    ltp::{
        prop_context::PropertyValue,
        table_context::{TableContext, TableContextInfo, TableRowColumnValue},
    },
    messaging::{folder::Folder as PstFolder, message::Message as PstMessage, store::Store},
    ndb::node_id::NodeId,
};

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe inventory for the teaspoon Outlook message miner (PST or MSG)"
)]
struct Args {
    /// A .pst file, a single .msg file, or a directory of .msg files to inspect.
    input: PathBuf,
}

enum InputKind {
    Pst,
    Msg {
        files: Vec<PathBuf>,
        /// Subdirectories found directly inside the scanned directory but
        /// not descended into (the scan is deliberately non-recursive).
        /// Surfaced in diagnostic output so this scoping choice is visible
        /// to whoever reads the output, not just to whoever reads the
        /// source -- a silent gap here would be the same kind of loss of
        /// transparency the zero-byte/HTML fixes exist to avoid.
        subdirectories_skipped: u64,
    },
}

/// Classifies the input by extension (or, for a directory, by scanning for
/// `.msg` files directly inside it -- not recursive). Never includes the
/// input path itself in any error message, consistent with the rest of
/// this tool's privacy-safe diagnostic output.
fn classify_input(path: &Path) -> Result<InputKind> {
    if path.is_dir() {
        let mut files = Vec::new();
        let mut subdirectories_skipped = 0u64;

        for entry in std::fs::read_dir(path)
            .context("failed to read input directory")?
            .filter_map(|entry| entry.ok())
        {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                subdirectories_skipped += 1;
                continue;
            }
            let is_msg = entry_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("msg"))
                .unwrap_or(false);
            if is_msg {
                files.push(entry_path);
            }
        }
        files.sort();
        if files.is_empty() {
            anyhow::bail!("input directory contains no .msg files");
        }
        return Ok(InputKind::Msg {
            files,
            subdirectories_skipped,
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("pst") => Ok(InputKind::Pst),
        Some("msg") => Ok(InputKind::Msg {
            files: vec![path.to_path_buf()],
            subdirectories_skipped: 0,
        }),
        _ => anyhow::bail!(
            "unsupported input: expected a .pst file, a .msg file, or a directory of .msg files"
        ),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    match classify_input(&args.input)? {
        InputKind::Pst => run_pst_diagnostic(&args.input),
        InputKind::Msg {
            files,
            subdirectories_skipped,
        } => run_msg_diagnostic(&files, subdirectories_skipped),
    }
}

// =============================================================================
// Shared: MS-OXRTFEX "RTF containing encapsulated HTML" detection
// =============================================================================

/// The MS-OXRTFEX control word a de-encapsulating reader uses to recognize
/// RTF containing encapsulated HTML. Per that specification, a reader
/// finding this control word "SHOULD conclude the RTF document contains
/// encapsulated HTML and stop further inspection" -- its presence alone is
/// the specification-sanctioned signal, not a heuristic.
///
/// Shared by both the PST and MSG diagnostics so they apply identically
/// strict detection, rather than two different signals that can silently
/// drift apart -- which is exactly what happened here: the MSG side
/// originally (2026-09-07) trusted `msg_parser::Outlook::html_from_rtf()`'s
/// mere non-emptiness as "HTML was found," without confirming that method
/// itself gates on this control word. It turned out not to: tested against
/// `RTF_message.msg` (confirmed, via Outlook's own View Source feature, to
/// be genuinely RTF-authored content -- its HTML view carries an explicit
/// `<!-- Converted from text/rtf format -->` comment and an
/// `MS Exchange Server` generator tag, meaning Exchange generated that HTML
/// at render time for display, which is an unrelated mechanism from
/// MS-OXRTFEX encapsulation), `html_from_rtf()` still returned non-empty
/// content (2026-09-13). Both diagnostics now check for this literal
/// control word directly against decompressed RTF bytes instead of
/// trusting either crate's own higher-level "give me HTML" convenience
/// method.
const FROMHTML_MARKER: &[u8] = b"\\fromhtml1";

/// Returns whether decompressed RTF bytes contain the FROMHTML control
/// word. A plain byte search, not a UTF-8/String conversion: RTF is an
/// ASCII-based control-word format (non-ASCII text is escaped as `\'XX`
/// hex sequences), so searching raw bytes avoids any encoding-conversion
/// question entirely.
fn rtf_bytes_contain_fromhtml(rtf_bytes: &[u8]) -> bool {
    rtf_bytes
        .windows(FROMHTML_MARKER.len())
        .any(|window| window == FROMHTML_MARKER)
}

// =============================================================================
// PST diagnostic (M1, unchanged in behavior from v0.1.4.3 except body-flag
// detection, corrected 2026-09-13)
// =============================================================================

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

fn run_pst_diagnostic(path: &Path) -> Result<()> {
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

    println!("inventory=privacy_safe");
    println!("input_kind=pst");
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
    println!("bodies_html_native={}", totals.bodies_html_native);
    println!("bodies_html_via_rtf={}", totals.bodies_html_via_rtf);
    println!("bodies_rtf={}", totals.bodies_rtf);
    println!(
        "rtf_decompression_errors={}",
        totals.rtf_decompression_errors
    );

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
    println!("recipients_type_other={}", totals.recipients_type_other);
    println!("recipients_type_unknown={}", totals.recipients_type_unknown);

    // --- P4b: attachment classification -------------------------------------
    println!(
        "attachment_row_read_errors={}",
        totals.attachment_row_read_errors
    );
    println!("attachments_zero_byte={}", totals.attachments_zero_byte);
    println!(
        "attachments_zero_size_other_method={}",
        totals.attachments_zero_size_other_method
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

    Ok(())
}

#[derive(Default)]
struct PstTotals {
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
    bodies_html_native: u64,
    bodies_html_via_rtf: u64,
    bodies_rtf: u64,
    rtf_decompression_errors: u64,

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
    attachments_zero_size_other_method: u64,
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

fn walk_folder(store: &dyn Store, folder: &dyn PstFolder, totals: &mut PstTotals) -> Result<()> {
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

fn inspect_message(message: &dyn PstMessage, totals: &mut PstTotals) {
    let properties = message.properties();
    totals.property_values += properties.iter().count() as u64;

    record_message_class(totals, properties.message_class());

    // HTML detection has two layers, exactly mirroring the MSG-side fix
    // (2026-09-07, corrected 2026-09-13) and the confirmed finding
    // (2026-09-13) that this dependency has the identical blind spot:
    // many real messages have no native PidTagBodyHtml property at all --
    // Outlook instead encapsulates the HTML inside PidTagRtfCompressed per
    // MS-OXRTFEX, detectable via the FROMHTML control word (see
    // [`FROMHTML_MARKER`]). This is a presence-only check: tsp never needs
    // the extracted HTML/RTF content itself, only whether this marker
    // exists, so no RTF-to-HTML conversion is implemented here.
    let has_html_native = properties.get(PROP_BODY_HTML).is_some();
    let has_rtf = properties.get(PROP_RTF_COMPRESSED).is_some();
    let has_html_via_rtf = if has_html_native {
        false
    } else {
        match check_rtf_for_encapsulated_html(properties.get(PROP_RTF_COMPRESSED)) {
            RtfHtmlCheck::Decompressed { contains_fromhtml } => contains_fromhtml,
            RtfHtmlCheck::DecompressionFailed => {
                totals.rtf_decompression_errors += 1;
                false
            }
            RtfHtmlCheck::NoRtfProperty | RtfHtmlCheck::NotBinary => false,
        }
    };

    record_body_flags(
        totals,
        properties.get(PROP_BODY).is_some(),
        has_html_native,
        has_html_via_rtf,
        has_rtf,
    );

    inspect_recipients(message, totals);
    inspect_attachments(message, totals);
}

/// The result of checking `PidTagRtfCompressed` for MS-OXRTFEX HTML
/// encapsulation. Kept as distinct variants rather than a single bool so
/// "no RTF property at all" (the common, unremarkable case) is never
/// conflated with "RTF was present but failed to decompress" (a genuine
/// anomaly worth its own counter), consistent with this project's
/// no-silent-loss principle.
enum RtfHtmlCheck {
    NoRtfProperty,
    NotBinary,
    DecompressionFailed,
    Decompressed { contains_fromhtml: bool },
}

/// Checks whether `PidTagRtfCompressed`, if present, contains HTML content
/// encapsulated per MS-OXRTFEX (see [`FROMHTML_MARKER`] for why presence of
/// that one control word is the correct signal). Decompression uses
/// `compressed-rtf` (MS-OXRTFCP), maintained by the same author as
/// `outlook-pst`; its magic numbers and dictionary were independently
/// cross-checked against `msg_parser`'s own from-scratch implementation of
/// the same algorithm and match exactly. Never returns or exposes the
/// actual RTF or HTML content -- only whether this one marker is present.
///
/// `compressed_rtf::decompress_rtf` indexes into the first 16 bytes of its
/// input unconditionally as part of its own MS-OXRTFCP header read, and
/// panics rather than erroring if given fewer -- confirmed by reading the
/// crate's actual source rather than assumed from its signature. This
/// function guards that case explicitly so a truncated or corrupt
/// `PidTagRtfCompressed` value cannot crash `tsp`.
fn check_rtf_for_encapsulated_html(rtf_property: Option<&PropertyValue>) -> RtfHtmlCheck {
    let Some(value) = rtf_property else {
        return RtfHtmlCheck::NoRtfProperty;
    };
    let PropertyValue::Binary(binary) = value else {
        return RtfHtmlCheck::NotBinary;
    };
    let buffer = binary.buffer();
    // `compressed_rtf::decompress_rtf` indexes into the first 16 bytes of
    // its input unconditionally (its own MS-OXRTFCP header read) and
    // panics if given fewer -- confirmed by reading the crate's actual
    // source, not assumed from its signature. Guarded here rather than
    // letting a truncated or corrupt property crash tsp outright.
    if buffer.len() < 16 {
        return RtfHtmlCheck::DecompressionFailed;
    }
    match compressed_rtf::decompress_rtf(buffer) {
        Ok(rtf) => RtfHtmlCheck::Decompressed {
            contains_fromhtml: rtf_bytes_contain_fromhtml(rtf.as_bytes()),
        },
        Err(_) => RtfHtmlCheck::DecompressionFailed,
    }
}

/// Records a message-class observation. `class` is `Err` when the message
/// has no readable `PidTagMessageClass` property; that is counted separately
/// rather than silently dropped, consistent with the project's no-silent-loss
/// principle (ADR: loss-aware-normalized-representation).
fn record_message_class(totals: &mut PstTotals, class: std::io::Result<String>) {
    match class {
        Ok(class) => *totals.message_classes.entry(class).or_insert(0) += 1,
        Err(_) => totals.message_class_read_errors += 1,
    }
}

/// Records body-*availability* only. Never receives or touches actual body
/// content -- callers must pass presence booleans, not the property values.
///
/// Interpretation caveat: `has_plain` reflects only that `PidTagBody`
/// exists, not that the message was *authored* in plain text. Outlook
/// commonly populates a plain-text compatibility mirror alongside an
/// HTML- or RTF-authored body regardless of how the message was actually
/// composed, so `bodies_plain` is expected to run high even on a mailbox
/// with little genuinely plain-text-only content. (Confirmed 2026-09-13
/// as a real, not just inferred, characteristic of this project's PST
/// fixture, alongside the HTML-in-RTF finding below.)
///
/// `has_html_native` and `has_html_via_rtf` are tracked as distinct
/// signals, never silently merged, mirroring the MSG-side fix
/// (2026-09-07, corrected 2026-09-13) and the confirmed finding
/// (2026-09-13) that this dependency has the identical blind spot:
/// `bodies_html` alone would have undercounted real HTML content stored
/// only via MS-OXRTFEX encapsulation in the RTF body.
fn record_body_flags(
    totals: &mut PstTotals,
    has_plain: bool,
    has_html_native: bool,
    has_html_via_rtf: bool,
    has_rtf: bool,
) {
    if has_plain {
        totals.bodies_plain += 1;
    }
    if has_html_native {
        totals.bodies_html_native += 1;
    }
    if has_html_via_rtf {
        totals.bodies_html_via_rtf += 1;
    }
    if has_html_native || has_html_via_rtf {
        totals.bodies_html += 1;
    }
    if has_rtf {
        totals.bodies_rtf += 1;
    }
}

fn record_recipients(totals: &mut PstTotals, count: u64) {
    if count > 0 {
        totals.messages_with_recipients += 1;
    }
    totals.total_recipients += count;
    totals.max_recipients = totals.max_recipients.max(count);
}

fn record_attachments(totals: &mut PstTotals, count: u64) {
    if count > 0 {
        totals.messages_with_attachments += 1;
    }
    totals.total_attachments += count;
    totals.max_attachments = totals.max_attachments.max(count);
}

/// Buckets a single recipient row by its `PidTagRecipientType` value.
/// `None` means the property was missing or not a 32-bit integer on that row.
fn record_recipient_type(totals: &mut PstTotals, recipient_type: Option<i32>) {
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
fn record_attachment_method(totals: &mut PstTotals, method: Option<i32>) {
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

/// Records whether an attachment row's `PidTagAttachSize` was exactly zero
/// -- but only as a meaningful "empty file" signal when the attachment's
/// method is `by_value`. For every other method (embedded message, OLE,
/// by-reference), the attachment's real content lives outside this
/// property entirely, so a zero reading there is expected and structural,
/// not evidence of an empty file. These are tracked as a separate counter
/// rather than silently merged into `attachments_zero_byte`, which would
/// have repeated the same silent-conflation mistake the HTML detection fix
/// (2026-09-07) corrected on the MSG side. `None` (property missing or
/// unreadable) is not counted as zero-byte either way.
fn record_attachment_size(totals: &mut PstTotals, size: Option<i32>, method: Option<i32>) {
    if size != Some(0) {
        return;
    }
    match method {
        Some(ATTACH_METHOD_BY_VALUE) => totals.attachments_zero_byte += 1,
        _ => totals.attachments_zero_size_other_method += 1,
    }
}

/// Records presence (not content) of `PidTagAttachContentId`, a common but
/// not definitive signal that an attachment is referenced inline (e.g. an
/// inline image) rather than a standalone file attachment.
fn record_attachment_content_id_presence(totals: &mut PstTotals, has_content_id: bool) {
    if has_content_id {
        totals.attachments_with_content_id += 1;
    }
}

/// Locates the index of a column by MAPI property ID within a table's column
/// descriptors, so its value can be looked up per row via [`read_i32_at`] or
/// a presence check.
fn column_index(context: &TableContextInfo, prop_id: u16) -> Option<usize> {
    context
        .columns()
        .iter()
        .position(|c| c.prop_id() == prop_id)
}

/// Reads a single row's value at `column_idx` as a 32-bit integer, or `None`
/// if the property is absent on this row, the column doesn't exist, or the
/// value isn't a 32-bit integer. Never returns string/binary content.
///
/// Untested assumption: this always matches `PropertyValue::Integer32` for
/// `PidTagRecipientType`, `PidTagAttachMethod`, and `PidTagAttachSize` on
/// every PST this code has been run against so far -- every fixture
/// message happened to store these as that type. A PST that stored one of
/// these differently would fail the match arm below and silently fall
/// through to `None` (bucketed as "unknown" by callers), rather than
/// panicking or miscounting into the wrong bucket, so the failure mode is
/// safe. But the assumption itself has never been falsified because it
/// has never been tested against data that would break it.
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

fn inspect_recipients(message: &dyn PstMessage, totals: &mut PstTotals) {
    let Some(table_rc) = message.recipient_table() else {
        record_recipients(totals, 0);
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

        let recipient_type =
            type_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
        record_recipient_type(totals, recipient_type);
    }

    record_recipients(totals, count);
}

fn inspect_attachments(message: &dyn PstMessage, totals: &mut PstTotals) {
    let Some(table_rc) = message.attachment_table() else {
        record_attachments(totals, 0);
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
        record_attachment_size(totals, size, method);

        record_attachment_method(totals, method);

        let has_content_id = content_id_idx
            .map(|idx| column_has_value(&row_values, idx))
            .unwrap_or(false);
        record_attachment_content_id_presence(totals, has_content_id);
    }

    record_attachments(totals, count);
}

// =============================================================================
// MSG diagnostic (M2-P1/P2 equivalent; HTML detection corrected 2026-09-13)
// =============================================================================

/// PidTagAttachMethod values as exposed by `msg_parser`'s `attach_method`
/// field. Same MAPI vocabulary as the PST side (MS-OXCMSG), but msg_parser
/// does not currently expose the by-reference variants (2/3/4) separately,
/// so they fall into `attachments_method_other` here if ever encountered.
const MSG_ATTACH_METHOD_BY_VALUE: u32 = 1;
const MSG_ATTACH_METHOD_EMBEDDED_MESSAGE: u32 = 5;
const MSG_ATTACH_METHOD_OLE: u32 = 6;

fn run_msg_diagnostic(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    println!("inventory=privacy_safe");
    println!("input_kind=msg");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");

    let mut totals = MsgTotals::default();

    for file in files {
        match Outlook::from_path(file) {
            Ok(outlook) => inspect_msg(&outlook, &mut totals),
            Err(_) => totals.open_errors += 1,
        }
    }

    println!("open_errors={}", totals.open_errors);

    println!("message_class_missing={}", totals.message_class_missing);
    for (class, count) in &totals.message_classes {
        println!("message_class class={class} count={count}");
    }

    println!("bodies_plain={}", totals.bodies_plain);
    println!("bodies_html={}", totals.bodies_html);
    println!("bodies_html_native={}", totals.bodies_html_native);
    println!("bodies_html_via_rtf={}", totals.bodies_html_via_rtf);
    println!("bodies_rtf={}", totals.bodies_rtf);
    println!(
        "rtf_decompression_errors={}",
        totals.rtf_decompression_errors
    );

    println!(
        "messages_with_recipients={}",
        totals.messages_with_recipients
    );
    println!("recipients_to={}", totals.recipients_to);
    println!("recipients_cc={}", totals.recipients_cc);
    println!("recipients_bcc={}", totals.recipients_bcc);
    println!("max_recipients_on_a_message={}", totals.max_recipients);

    println!(
        "messages_with_attachments={}",
        totals.messages_with_attachments
    );
    println!("total_attachments={}", totals.total_attachments);
    println!("max_attachments_on_a_message={}", totals.max_attachments);
    println!("attachments_zero_byte={}", totals.attachments_zero_byte);
    println!(
        "attachments_zero_size_other_method={}",
        totals.attachments_zero_size_other_method
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
    // real file, not just per the crate's documentation. One level deep only
    // -- deeper recursion is explicitly deferred, not attempted here.
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

    Ok(())
}

#[derive(Default)]
struct MsgTotals {
    open_errors: u64,

    message_classes: BTreeMap<String, u64>,
    message_class_missing: u64,

    bodies_plain: u64,
    bodies_html: u64,
    bodies_html_native: u64,
    bodies_html_via_rtf: u64,
    bodies_rtf: u64,
    rtf_decompression_errors: u64,

    messages_with_recipients: u64,
    recipients_to: u64,
    recipients_cc: u64,
    recipients_bcc: u64,
    max_recipients: u64,

    messages_with_attachments: u64,
    total_attachments: u64,
    max_attachments: u64,
    attachments_zero_byte: u64,
    attachments_zero_size_other_method: u64,
    attachments_with_content_id: u64,
    attachments_method_by_value: u64,
    attachments_method_embedded_message: u64,
    attachments_method_ole: u64,
    attachments_method_other: u64,

    embedded_messages_opened: u64,
    embedded_message_open_errors: u64,
    embedded_message_classes: BTreeMap<String, u64>,
}

fn inspect_msg(outlook: &Outlook, totals: &mut MsgTotals) {
    record_msg_class(totals, &outlook.message_class);

    // HTML detection, corrected 2026-09-13: this used to trust
    // `Outlook::html_from_rtf()`'s mere non-emptiness as "HTML was found."
    // That was wrong -- proven wrong by `RTF_message.msg`, a message
    // confirmed genuinely RTF-authored (via Outlook's own View Source
    // feature showing an explicit "Converted from text/rtf format" /
    // "MS Exchange Server" render-time conversion, not authored HTML) for
    // which `html_from_rtf()` still returned non-empty content. It
    // evidently does not gate on the FROMHTML control word the way the
    // specification requires for a real detection signal. This now checks
    // the decompressed RTF bytes directly for that control word, via the
    // same shared check the PST side uses (see [`FROMHTML_MARKER`]),
    // rather than trusting either crate's own higher-level convenience
    // method.
    let has_html_native = !outlook.html.is_empty();
    let has_rtf = !outlook.rtf_compressed.is_empty();
    let has_html_via_rtf = if has_html_native {
        false
    } else if has_rtf {
        match outlook.rtf_decompressed() {
            Some(bytes) => rtf_bytes_contain_fromhtml(&bytes),
            None => {
                totals.rtf_decompression_errors += 1;
                false
            }
        }
    } else {
        false
    };

    record_msg_body_flags(
        totals,
        !outlook.body.is_empty(),
        has_html_native,
        has_html_via_rtf,
        has_rtf,
    );

    record_msg_recipients(
        totals,
        outlook.to.len() as u64,
        outlook.cc.len() as u64,
        outlook.bcc.len() as u64,
    );

    let mut attachment_count = 0u64;
    for attach in &outlook.attachments {
        attachment_count += 1;

        record_msg_attachment_size(totals, attach.payload_bytes.len(), attach.attach_method);
        record_msg_attachment_method(totals, attach.attach_method);
        record_msg_attachment_content_id(totals, !attach.content_id.is_empty());

        match attach.as_message() {
            Some(Ok(nested)) => {
                totals.embedded_messages_opened += 1;
                record_embedded_message_class(totals, &nested.message_class);
            }
            Some(Err(_)) => totals.embedded_message_open_errors += 1,
            None => {}
        }
    }
    record_msg_attachments(totals, attachment_count);
}

fn record_msg_class(totals: &mut MsgTotals, class: &str) {
    if class.is_empty() {
        totals.message_class_missing += 1;
    } else {
        *totals.message_classes.entry(class.to_string()).or_insert(0) += 1;
    }
}

/// Records body-*availability* only, distinguishing native HTML (a real
/// `PidTagBodyHtml` property) from HTML recovered by decoding it out of the
/// RTF body -- these are different levels of confidence in the result and
/// are never merged into a single ambiguous signal. `bodies_html` is a
/// convenience "was HTML detected via either path" total.
///
/// Interpretation caveat: `has_plain` reflects only that `outlook.body` is
/// non-empty, not that the message was *authored* in plain text -- see the
/// identical caveat on the PST-side `record_body_flags`, which this
/// mirrors. Every fixture message observed so far has `bodies_plain` set
/// regardless of its real authored format.
fn record_msg_body_flags(
    totals: &mut MsgTotals,
    has_plain: bool,
    has_html_native: bool,
    has_html_via_rtf: bool,
    has_rtf: bool,
) {
    if has_plain {
        totals.bodies_plain += 1;
    }
    if has_html_native {
        totals.bodies_html_native += 1;
    }
    if has_html_via_rtf {
        totals.bodies_html_via_rtf += 1;
    }
    if has_html_native || has_html_via_rtf {
        totals.bodies_html += 1;
    }
    if has_rtf {
        totals.bodies_rtf += 1;
    }
}

/// Records recipient counts by type. Structural gap, not a bug: `msg_parser`
/// exposes only `to`/`cc`/`bcc` on `Outlook`, with no equivalent of the
/// PST side's `PidTagRecipientType` "ORIG" bucket (a recipient recorded as
/// the sender, MS-OXOMSG value 0). If a `.msg` file ever had an
/// ORIG-classified recipient, there is currently no way to detect it
/// through this crate's public API -- it would simply not be counted
/// anywhere. This asymmetry with the PST-side diagnostic (which does track
/// ORIG, currently always at zero) is accepted as a known limitation, not
/// scheduled for a fix, since ORIG recipients are a rare edge case and no
/// fixture evidence has shown one to even test against.
fn record_msg_recipients(totals: &mut MsgTotals, to: u64, cc: u64, bcc: u64) {
    if to + cc + bcc > 0 {
        totals.messages_with_recipients += 1;
    }
    totals.recipients_to += to;
    totals.recipients_cc += cc;
    totals.recipients_bcc += bcc;
    totals.max_recipients = totals.max_recipients.max(to + cc + bcc);
}

fn record_msg_attachments(totals: &mut MsgTotals, count: u64) {
    if count > 0 {
        totals.messages_with_attachments += 1;
    }
    totals.total_attachments += count;
    totals.max_attachments = totals.max_attachments.max(count);
}

/// Records whether an attachment's byte payload was exactly zero -- but
/// only as a meaningful "empty file" signal when the attachment's method
/// is `by_value`. For every other method (embedded message, OLE), the
/// attachment's real content lives outside `payload_bytes` entirely, so a
/// zero reading there is expected and structural, not evidence of an empty
/// file. Tracked as a separate counter, mirroring the same fix applied to
/// the PST side.
fn record_msg_attachment_size(totals: &mut MsgTotals, size: usize, method: u32) {
    if size != 0 {
        return;
    }
    if method == MSG_ATTACH_METHOD_BY_VALUE {
        totals.attachments_zero_byte += 1;
    } else {
        totals.attachments_zero_size_other_method += 1;
    }
}

fn record_msg_attachment_method(totals: &mut MsgTotals, method: u32) {
    match method {
        MSG_ATTACH_METHOD_BY_VALUE => totals.attachments_method_by_value += 1,
        MSG_ATTACH_METHOD_EMBEDDED_MESSAGE => totals.attachments_method_embedded_message += 1,
        MSG_ATTACH_METHOD_OLE => totals.attachments_method_ole += 1,
        _ => totals.attachments_method_other += 1,
    }
}

fn record_msg_attachment_content_id(totals: &mut MsgTotals, has_content_id: bool) {
    if has_content_id {
        totals.attachments_with_content_id += 1;
    }
}

fn record_embedded_message_class(totals: &mut MsgTotals, class: &str) {
    if !class.is_empty() {
        *totals
            .embedded_message_classes
            .entry(class.to_string())
            .or_insert(0) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut totals = PstTotals::default();
        record_body_flags(&mut totals, true, true, false, false);
        record_body_flags(&mut totals, true, false, false, false);

        assert_eq!(totals.bodies_plain, 2);
        assert_eq!(totals.bodies_html, 1);
        assert_eq!(totals.bodies_rtf, 0);
    }

    #[test]
    fn pst_html_native_and_via_rtf_are_tracked_separately_but_both_count_as_html() {
        let mut totals = PstTotals::default();
        // Native PidTagBodyHtml present.
        record_body_flags(&mut totals, false, true, false, false);
        // No native property, but HTML recovered via MS-OXRTFEX
        // encapsulation in the RTF body -- the case the 2026-09-13 fix
        // exists for.
        record_body_flags(&mut totals, false, false, true, true);

        assert_eq!(totals.bodies_html_native, 1);
        assert_eq!(totals.bodies_html_via_rtf, 1);
        assert_eq!(totals.bodies_html, 2);
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
        let garbage =
            PropertyValue::Binary(outlook_pst::ltp::prop_context::BinaryValue::new(vec![
                0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]));
        assert!(matches!(
            check_rtf_for_encapsulated_html(Some(&garbage)),
            RtfHtmlCheck::DecompressionFailed
        ));
    }

    #[test]
    fn recipient_counts_track_presence_total_and_max() {
        let mut totals = PstTotals::default();
        record_recipients(&mut totals, 0);
        record_recipients(&mut totals, 3);
        record_recipients(&mut totals, 1);

        assert_eq!(totals.messages_with_recipients, 2);
        assert_eq!(totals.total_recipients, 4);
        assert_eq!(totals.max_recipients, 3);
    }

    #[test]
    fn attachment_counts_track_presence_total_and_max() {
        let mut totals = PstTotals::default();
        record_attachments(&mut totals, 0);
        record_attachments(&mut totals, 2);
        record_attachments(&mut totals, 5);

        assert_eq!(totals.messages_with_attachments, 2);
        assert_eq!(totals.total_attachments, 7);
        assert_eq!(totals.max_attachments, 5);
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
        let mut totals = PstTotals::default();
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_BY_VALUE));
        record_attachment_size(&mut totals, Some(1024), Some(ATTACH_METHOD_BY_VALUE));
        record_attachment_size(&mut totals, None, Some(ATTACH_METHOD_BY_VALUE));

        assert_eq!(totals.attachments_zero_byte, 1);
        assert_eq!(totals.attachments_zero_size_other_method, 0);
    }

    #[test]
    fn zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
        let mut totals = PstTotals::default();
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_EMBEDDED_MESSAGE));
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_OLE));
        record_attachment_size(&mut totals, Some(0), None);

        assert_eq!(totals.attachments_zero_byte, 0);
        assert_eq!(totals.attachments_zero_size_other_method, 3);
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

    #[test]
    fn msg_body_flags_are_presence_only() {
        let mut totals = MsgTotals::default();
        record_msg_body_flags(&mut totals, true, false, false, true);
        record_msg_body_flags(&mut totals, false, true, false, false);

        assert_eq!(totals.bodies_plain, 1);
        assert_eq!(totals.bodies_html, 1);
        assert_eq!(totals.bodies_rtf, 1);
    }

    #[test]
    fn msg_html_native_and_via_rtf_are_tracked_separately_but_both_count_as_html() {
        let mut totals = MsgTotals::default();
        // Native HTML property present.
        record_msg_body_flags(&mut totals, false, true, false, false);
        // No native property, but HTML recovered from RTF encapsulation
        // (MS-OXRTFEX) -- the case this fix exists for.
        record_msg_body_flags(&mut totals, false, false, true, true);

        assert_eq!(totals.bodies_html_native, 1);
        assert_eq!(totals.bodies_html_via_rtf, 1);
        assert_eq!(totals.bodies_html, 2);
    }

    #[test]
    fn msg_recipients_are_split_by_type_with_presence_and_max() {
        let mut totals = MsgTotals::default();
        record_msg_recipients(&mut totals, 0, 0, 0);
        record_msg_recipients(&mut totals, 1, 2, 1);
        record_msg_recipients(&mut totals, 1, 0, 0);

        assert_eq!(totals.messages_with_recipients, 2);
        assert_eq!(totals.recipients_to, 2);
        assert_eq!(totals.recipients_cc, 2);
        assert_eq!(totals.recipients_bcc, 1);
        assert_eq!(totals.max_recipients, 4);
    }

    #[test]
    fn msg_attachment_methods_are_bucketed_correctly() {
        let mut totals = MsgTotals::default();
        record_msg_attachment_method(&mut totals, MSG_ATTACH_METHOD_BY_VALUE);
        record_msg_attachment_method(&mut totals, MSG_ATTACH_METHOD_EMBEDDED_MESSAGE);
        record_msg_attachment_method(&mut totals, MSG_ATTACH_METHOD_OLE);
        record_msg_attachment_method(&mut totals, 99);

        assert_eq!(totals.attachments_method_by_value, 1);
        assert_eq!(totals.attachments_method_embedded_message, 1);
        assert_eq!(totals.attachments_method_ole, 1);
        assert_eq!(totals.attachments_method_other, 1);
    }

    #[test]
    fn msg_zero_byte_attachment_is_counted() {
        let mut totals = MsgTotals::default();
        record_msg_attachment_size(&mut totals, 0, MSG_ATTACH_METHOD_BY_VALUE);
        record_msg_attachment_size(&mut totals, 117, MSG_ATTACH_METHOD_BY_VALUE);

        assert_eq!(totals.attachments_zero_byte, 1);
        assert_eq!(totals.attachments_zero_size_other_method, 0);
    }

    #[test]
    fn msg_zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
        let mut totals = MsgTotals::default();
        record_msg_attachment_size(&mut totals, 0, MSG_ATTACH_METHOD_EMBEDDED_MESSAGE);
        record_msg_attachment_size(&mut totals, 0, MSG_ATTACH_METHOD_OLE);

        assert_eq!(totals.attachments_zero_byte, 0);
        assert_eq!(totals.attachments_zero_size_other_method, 2);
    }

    #[test]
    fn msg_embedded_message_class_ignores_empty() {
        let mut totals = MsgTotals::default();
        record_embedded_message_class(&mut totals, "IPM.Note");
        record_embedded_message_class(&mut totals, "");

        assert_eq!(totals.embedded_message_classes.get("IPM.Note"), Some(&1));
        assert_eq!(totals.embedded_message_classes.len(), 1);
    }
}
