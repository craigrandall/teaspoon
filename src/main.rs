use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
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

    /// Use the experimental custom MS-OXMSG parser (raw CFB structural
    /// enumeration via the `cfb` crate) instead of `msg_parser` for .msg
    /// input. PST input is unaffected. This is a P1/P2-equivalent spike:
    /// it proves the container opens and enumerates its structure, and
    /// does not yet decode any property value.
    #[arg(long)]
    oxmsg: bool,
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
        } => {
            if args.oxmsg {
                run_oxmsg_diagnostic(&files, subdirectories_skipped)
            } else {
                run_msg_diagnostic(&files, subdirectories_skipped)
            }
        }
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
    println!(
        "rtf_decompressed_bytes_total={}",
        totals.rtf_decompressed_bytes_total
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
    /// Sum of decompressed-RTF byte lengths across every message where
    /// decompression succeeded (regardless of whether the FROMHTML marker
    /// was found). Never the content itself -- a size-only diagnostic
    /// added 2026-09-14 specifically to let the PST and MSG sides be
    /// compared against each other when they disagree on the same
    /// underlying message, without ever printing or comparing content.
    rtf_decompressed_bytes_total: u64,

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
            RtfHtmlCheck::Decompressed {
                contains_fromhtml,
                decompressed_bytes,
            } => {
                totals.rtf_decompressed_bytes_total += decompressed_bytes as u64;
                contains_fromhtml
            }
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
    Decompressed {
        contains_fromhtml: bool,
        decompressed_bytes: usize,
    },
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
            decompressed_bytes: rtf.len(),
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

        let recipient_type = type_idx.and_then(|idx| read_i32_at(table, context, &row_values, idx));
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
        "rtf_decompressed_bytes_total={}",
        totals.rtf_decompressed_bytes_total
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
    rtf_decompressed_bytes_total: u64,

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
            Some(bytes) => {
                totals.rtf_decompressed_bytes_total += bytes.len() as u64;
                rtf_bytes_contain_fromhtml(&bytes)
            }
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

// =============================================================================
// Custom MS-OXMSG parser groundwork (experimental, opt-in via --oxmsg)
// =============================================================================
//
// P1/P2-equivalent spike, mirroring the shape M1's PST spike started with:
// prove a real .msg container opens via a generic, non-Outlook-specific CFB
// (MS-CFB / Compound File Binary) reader, and enumerate its structure --
// message class not yet decoded, no property value read, nothing beyond
// presence/name/size. This exists because msg_parser has no raw/generic
// property-iteration equivalent to outlook-pst's `.get(id)`/`.iter()`,
// which is the one structural inconsistency remaining between teaspoon's
// two format adapters (see the 2026-09-14 comparative analysis in project
// correspondence). The `cfb` crate (crates.io, MIT) handles the generic
// container-parsing layer; only the MS-OXMSG-specific naming convention
// below is teaspoon's own.
//
// MS-OXMSG stores every message property as one of two things inside the
// CFB container:
// - fixed-length properties, packed together inside a single stream named
//   `__properties_version1.0` (not yet decoded here -- its byte length is
//   reported, not its packed contents);
// - variable-length properties (strings, binary, and variable-length
//   multi-valued values) use streams named
//   `__substg1.0_PPPPTTTT`, where PPPP is the 4-hex-digit property ID and
//   TTTT is the 4-hex-digit property type. Variable-length multi-valued
//   values add a zero-based `-NNNNNNNN` value-index suffix.
// Recipients and attachments each get their own numbered sub-storage
// (`__recip_version1.0_#NNNNNNNN`, `__attach_version1.0_#NNNNNNNN`).
// Embedded/custom-object storage is represented by
// `__substg1.0_3701000D`, while named (non-standard) properties get a
// dedicated `__nameid_version1.0` storage. All of this is name/size/count
// only -- never content.

fn run_oxmsg_diagnostic(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    println!("inventory=privacy_safe");
    println!("input_kind=msg_oxmsg");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");

    let mut totals = OxmsgTotals::default();

    for file in files {
        match cfb::open(file) {
            Ok(mut comp) => inspect_oxmsg(&mut comp, &mut totals),
            Err(_) => totals.open_errors += 1,
        }
    }

    println!("open_errors={}", totals.open_errors);
    println!("total_entries={}", totals.total_entries);
    println!("root_entries_total={}", totals.root_entries_total);
    println!(
        "recognized_entries_total={}",
        totals.recognized_entries_total
    );
    println!(
        "entry_accounting_gap_total={}",
        totals.entry_accounting_gap_total()
    );
    println!("has_properties_stream={}", totals.has_properties_stream);
    println!(
        "properties_stream_entries_total={}",
        totals.properties_stream_entries_total
    );
    println!(
        "properties_stream_bytes_total={}",
        totals.properties_stream_bytes_total
    );
    println!("property_streams_total={}", totals.property_streams_total);
    println!(
        "indexed_property_streams_total={}",
        totals.indexed_property_streams_total
    );
    println!(
        "embedded_object_storages_total={}",
        totals.embedded_object_storages_total
    );
    println!(
        "attachment_storages_total={}",
        totals.attachment_storages_total
    );
    println!(
        "recipient_storages_total={}",
        totals.recipient_storages_total
    );
    println!(
        "named_property_storages_total={}",
        totals.named_property_storages_total
    );
    // Keep the original presence field for consumers that only need a
    // compatibility-friendly yes/no result.
    println!(
        "has_named_property_storage={}",
        totals.named_property_storages_total > 0
    );
    println!(
        "unrecognized_entries_total={}",
        totals.unrecognized_entries_total
    );
    println!(
        "embedded_object_storages_message_shaped_total={}",
        totals.embedded_object_storages_message_shaped_total
    );
    println!(
        "embedded_object_storages_custom_total={}",
        totals.embedded_object_storages_custom_total
    );
    for ((shape, clsid), count) in &totals.embedded_object_storages_by_shape {
        println!("embedded_object_storage shape={shape} clsid={clsid} count={count}");
    }
    println!(
        "opaque_payload_entries_total={}",
        totals.opaque_payload_entries_total
    );
    for ((object_kind, below), count) in &totals.opaque_payload_entries {
        println!(
            "opaque_payload_entry kind={} depth_below_payload={below} count={count}",
            object_kind.as_str()
        );
    }
    for ((object_kind, depth, name_shape, ancestry), count) in &totals.unrecognized_entries {
        println!(
            "unrecognized_entry kind={} depth={} name_shape={} ancestry={ancestry} count={count}",
            object_kind.as_str(),
            depth,
            name_shape.as_str()
        );
    }
    for (mismatch, count) in &totals.recognized_name_type_mismatches {
        println!(
            "recognized_name_type_mismatch kind={} count={count}",
            mismatch.as_str()
        );
    }
    for (scope, count) in &totals.properties_streams_by_scope {
        println!("properties_stream scope={} count={count}", scope.as_str());
    }
    for (scope, count) in &totals.property_streams_by_scope {
        println!("property_stream scope={} count={count}", scope.as_str());
    }
    for ((scope, prop_id), count) in &totals.property_id_counts_by_scope {
        println!(
            "property_id scope={} id=0x{prop_id:04X} count={count}",
            scope.as_str()
        );
    }
    for (prop_id, count) in &totals.property_id_counts {
        println!("property_id id=0x{prop_id:04X} count={count}");
    }
    println!(
        "properties_entries_total={}",
        totals.properties_entries_total
    );
    println!(
        "properties_entries_fixed_inline_total={}",
        totals.properties_entries_fixed_inline_total
    );
    println!(
        "properties_entries_variable_single_total={}",
        totals.properties_entries_variable_single_total
    );
    println!(
        "properties_entries_variable_multivalued_total={}",
        totals.properties_entries_variable_multivalued_total
    );
    println!(
        "properties_stream_too_short_for_header_total={}",
        totals.properties_stream_too_short_for_header_total
    );
    println!(
        "properties_stream_trailing_bytes_total={}",
        totals.properties_stream_trailing_bytes_total
    );
    println!(
        "properties_stream_unexpected_scope_total={}",
        totals.properties_stream_unexpected_scope_total
    );
    println!(
        "properties_stream_read_errors={}",
        totals.properties_stream_read_errors
    );
    for ((scope, property_type), count) in &totals.property_type_counts_by_scope {
        println!(
            "property_type scope={} type=0x{property_type:04X} count={count}",
            scope.as_str()
        );
    }
    for (flags, count) in &totals.property_entry_flags_counts {
        println!("property_entry_flags value=0x{flags:X} count={count}");
    }
    for (reserved, count) in &totals.attach_data_object_reserved_counts {
        println!("attach_data_object_entry reserved=0x{reserved:02X} count={count}");
    }
    println!(
        "attach_data_object_size_sentinel_mismatches={}",
        totals.attach_data_object_size_sentinel_mismatches
    );
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
    println!(
        "attach_data_object_reserved_vs_embedded_object_storage_gap={}",
        reserved_embedded as i64 - totals.embedded_object_storages_message_shaped_total as i64
    );
    println!(
        "attach_data_object_reserved_vs_custom_object_storage_gap={}",
        reserved_storage as i64 - totals.embedded_object_storages_custom_total as i64
    );
    println!(
        "fixed_boolean_invalid_encoding_total={}",
        totals.fixed_boolean_invalid_encoding_total
    );
    println!(
        "fixed_float_non_finite_total={}",
        totals.fixed_float_non_finite_total
    );
    println!(
        "variable_value_stream_found_total={}",
        totals.variable_value_stream_found_total
    );
    println!(
        "variable_value_stream_missing_total={}",
        totals.variable_value_stream_missing_total
    );
    println!(
        "variable_value_size_mismatch_total={}",
        totals.variable_value_size_mismatch_total
    );
    println!(
        "variable_value_odd_utf16_length_total={}",
        totals.variable_value_odd_utf16_length_total
    );
    println!(
        "variable_unicode_decode_errors_total={}",
        totals.variable_unicode_decode_errors_total
    );
    println!(
        "variable_string8_undefined_byte_total={}",
        totals.variable_string8_undefined_byte_total
    );
    println!(
        "variable_clsid_wrong_length_total={}",
        totals.variable_clsid_wrong_length_total
    );
    println!(
        "named_properties_seen_total={}",
        totals.named_properties_seen_total
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
        "named_properties_guid_out_of_range_total={}",
        totals.named_properties_guid_out_of_range_total
    );
    println!(
        "named_properties_string_kind_total={}",
        totals.named_properties_string_kind_total
    );
    println!(
        "named_properties_numeric_kind_total={}",
        totals.named_properties_numeric_kind_total
    );
    for (set, count) in &totals.named_property_sets {
        println!("named_property_set set={set} count={count}");
    }
    for (lid, count) in &totals.named_property_numeric_lids {
        println!("named_property_numeric_lid lid=0x{lid:04X} count={count}");
    }
    println!(
        "named_properties_index_mismatch_total={}",
        totals.named_properties_index_mismatch_total
    );

    Ok(())
}

#[derive(Default)]
struct OxmsgTotals {
    open_errors: u64,
    total_entries: u64,
    root_entries_total: u64,
    recognized_entries_total: u64,

    has_properties_stream: u64,
    /// Every `__properties_version1.0` entry, including property streams
    /// inside recipient, attachment, and embedded-message storages.
    properties_stream_entries_total: u64,
    properties_stream_bytes_total: u64,

    property_streams_total: u64,
    /// Property streams whose names include the zero-based value index used
    /// by variable-length multiple-valued properties.
    indexed_property_streams_total: u64,
    /// Aggregate counts by MAPI property ID across every file scanned.
    /// Property IDs are a bounded, standard MAPI vocabulary (like message
    /// class names elsewhere in this codebase), not user content, so
    /// reporting them by ID does not violate the privacy-safe design.
    /// It excludes named-property-storage streams.
    property_id_counts: BTreeMap<u16, u64>,
    /// Property-stream counts separated by the containing MS-OXMSG object
    /// scope, so message/recipient/attachment/embedded/named-property
    /// structures are not conflated.
    property_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,
    /// Property ID counts separated by the containing MS-OXMSG object scope.
    property_id_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,
    /// Property-stream counts separated by the containing MS-OXMSG object
    /// scope. The named-property mapping storage is intentionally distinct
    /// from ordinary Message/Recipient/Attachment property scopes.
    properties_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,

    attachment_storages_total: u64,
    recipient_storages_total: u64,
    named_property_storages_total: u64,
    embedded_object_storages_total: u64,

    /// Entries whose name matched none of the known MS-OXMSG conventions.
    /// Counted, never silently dropped, consistent with this project's
    /// no-silent-loss principle -- a nonzero count here means either an
    /// MS-OXMSG structure this parser doesn't know about yet, or a real
    /// anomaly worth a closer look.
    unrecognized_entries_total: u64,
    /// Privacy-safe structural breakdown. Names and paths are never emitted.
    unrecognized_entries: BTreeMap<(CfbObjectKind, u64, UnrecognizedNameShape, String), u64>,
    /// A recognized MS-OXMSG name whose CFB object type is unexpected.
    recognized_name_type_mismatches: BTreeMap<RecognizedNameTypeMismatch, u64>,

    /// Entries beneath a custom (non-message-shaped) `__substg1.0_3701000D`
    /// storage. Their names are defined by the producing application, not
    /// MS-OXMSG (MS-OXMSG "Custom Attachment Storage"), so they are counted
    /// as opaque payload rather than matched against MS-OXMSG names.
    opaque_payload_entries_total: u64,
    /// (object kind, depth below the payload root) -> count.
    opaque_payload_entries: BTreeMap<(CfbObjectKind, u64), u64>,
    embedded_object_storages_message_shaped_total: u64,
    embedded_object_storages_custom_total: u64,
    /// (shape, storage CLSID) -> count. CLSIDs are a bounded class-identifier
    /// vocabulary, not user content.
    embedded_object_storages_by_shape: BTreeMap<(&'static str, String), u64>,

    // --- Property-type/value decoding (privacy-safe first slice) ---------
    /// Every fixed-length entry decoded from a `__properties_version1.0`
    /// stream's entry array. Entry values are never read or reported --
    /// only structural fields (type, ID, flags, and, for variable-length
    /// entries, size/reserved).
    properties_entries_total: u64,
    properties_entries_fixed_inline_total: u64,
    properties_entries_variable_single_total: u64,
    properties_entries_variable_multivalued_total: u64,
    /// A stream shorter than the header size expected for its scope.
    properties_stream_too_short_for_header_total: u64,
    /// A stream whose length past the header isn't an exact multiple of 16.
    properties_stream_trailing_bytes_total: u64,
    /// A properties stream in a scope with no defined header size (per
    /// MS-OXMSG this should never be Named Property Mapping storage).
    properties_stream_unexpected_scope_total: u64,
    properties_stream_read_errors: u64,
    /// (scope, raw property type incl. the 0x1000 multi-value bit) -> count.
    /// Property types are a bounded MAPI vocabulary (MS-OXCDATA 2.11.1), not
    /// user content.
    property_type_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,
    /// Property Entry flags are a 3-bit MS-OXMSG vocabulary (mandatory /
    /// readable / writable), not user content.
    property_entry_flags_counts: BTreeMap<u32, u64>,
    /// Reserved-field values seen on the attachment-scope
    /// PidTagAttachDataObject (0x3701, PT_OBJECT) entry. Per MS-OXMSG
    /// 2.4.2.2, this is 0x01 for an embedded-message attachment and 0x04 for
    /// a storage (OLE/custom) attachment -- an independent, property-level
    /// cross-check of the CFB-structural message-shaped/custom
    /// classification established in M2.x.
    attach_data_object_reserved_counts: BTreeMap<u32, u64>,
    /// Per spec this entry's Size field MUST be 0xFFFFFFFF; count any that
    /// aren't, rather than assuming.
    attach_data_object_size_sentinel_mismatches: u64,

    // --- Fixed-value and variable-value structural checks -----------------
    fixed_boolean_invalid_encoding_total: u64,
    /// A decoded PT_FLOAT/PT_DOUBLE/PT_APPTIME value that is NaN or
    /// infinite. Not necessarily invalid data on its own, but implausible
    /// for the values these types are normally used for (percentages,
    /// currency-like amounts, OLE Automation dates) -- worth investigating
    /// as a possible decode-path bug before assuming it's genuine.
    fixed_float_non_finite_total: u64,
    variable_value_stream_found_total: u64,
    variable_value_stream_missing_total: u64,
    variable_value_size_mismatch_total: u64,
    variable_value_odd_utf16_length_total: u64,
    /// PT_UNICODE bytes (already confirmed even-length) that still fail to
    /// decode as valid UTF-16 -- e.g. an unpaired surrogate.
    variable_unicode_decode_errors_total: u64,
    /// Bytes replaced with U+FFFD while decoding a PT_STRING8 value as
    /// Windows-1252 -- unverified against real data; see M3b.
    variable_string8_undefined_byte_total: u64,
    /// A PT_CLSID (0x0048) value stream whose length isn't exactly the 16
    /// bytes a GUID requires.
    variable_clsid_wrong_length_total: u64,
    // --- Named-property resolution -----------------------------------------
    named_properties_seen_total: u64,
    named_properties_map_missing_total: u64,
    named_properties_unresolvable_total: u64,
    named_properties_guid_out_of_range_total: u64,
    named_properties_string_kind_total: u64,
    named_properties_numeric_kind_total: u64,
    /// Property-set membership, by bounded label ("PS_MAPI",
    /// "PS_PUBLIC_STRINGS", a well-known PSETID name, or "custom").
    named_property_sets: BTreeMap<&'static str, u64>,
    /// Numeric LIDs are small application-defined integers, not content --
    /// same footing as a property ID.
    named_property_numeric_lids: BTreeMap<u32, u64>,
    /// A resolved entry whose own claimed Property Index doesn't match the
    /// array position it was looked up by. Per MS-OXMSG this MUST always
    /// match; a real permanent cross-check now that the bit layout is
    /// confirmed, rather than the disproven swap-hypothesis instrumentation
    /// it replaces.
    named_properties_index_mismatch_total: u64,
}

impl OxmsgTotals {
    /// Difference between every enumerated CFB entry and the classified
    /// categories. This remains signed so a future overcount is visible.
    fn entry_accounting_gap_total(&self) -> i64 {
        self.total_entries as i64
            - self.recognized_entries_total as i64
            - self.opaque_payload_entries_total as i64
            - self.unrecognized_entries_total as i64
    }
}

/// Decides only whether a property's value fits inline in a Property
/// Entry's 8-byte value field (MS-OXMSG 2.4.2.1) or lives in a separate
/// stream (2.4.2.2) -- never reads or reports the value itself. `base_type`
/// must already have the 0x1000 multi-value bit cleared by the caller.
fn is_fixed_length_base_type(base_type: u16) -> bool {
    matches!(
        base_type,
        0x0002 // PT_SHORT / PT_I2
            | 0x0003 // PT_LONG / PT_I4
            | 0x0004 // PT_FLOAT / PT_R4
            | 0x0005 // PT_DOUBLE / PT_R8
            | 0x0006 // PT_CURRENCY
            | 0x0007 // PT_APPTIME
            | 0x000A // PT_ERROR
            | 0x000B // PT_BOOLEAN
            | 0x0014 // PT_I8 / PT_LONGLONG
            | 0x0040 // PT_SYSTIME
    )
}

const PROPERTY_TYPE_MULTIVALUE_BIT: u16 = 0x1000;

fn property_type_is_multivalued(property_type: u16) -> bool {
    property_type & PROPERTY_TYPE_MULTIVALUE_BIT != 0
}

#[derive(Clone, Copy)]
enum PropertyEntryShape {
    /// Value stored inline in the entry's 8-byte value field.
    FixedInline,
    /// Value stored in a separate `__substg1.0_PPPPTTTT` stream.
    VariableSingle,
    /// Values stored in a separate, indexed set of
    /// `__substg1.0_PPPPTTTT-NNNNNNNN` streams.
    VariableMultivalued,
}

fn classify_property_entry_shape(property_type: u16) -> PropertyEntryShape {
    if property_type_is_multivalued(property_type) {
        PropertyEntryShape::VariableMultivalued
    } else if is_fixed_length_base_type(property_type) {
        PropertyEntryShape::FixedInline
    } else {
        PropertyEntryShape::VariableSingle
    }
}

/// The entry-array header size for a `__properties_version1.0` stream,
/// which depends on the containing object (MS-OXMSG 2.4.1.1/2.4.1.2, and
/// the attachment/recipient 8-byte reserved header). Named Property
/// Mapping storage has no property stream at all (2.4), so it has no
/// defined header size here.
fn properties_stream_header_len(scope: OxmsgEntryScope) -> Option<usize> {
    match scope {
        OxmsgEntryScope::Message => Some(32),
        OxmsgEntryScope::EmbeddedObject => Some(24),
        OxmsgEntryScope::Attachment | OxmsgEntryScope::Recipient => Some(8),
        OxmsgEntryScope::NamedPropertyStorage => None,
    }
}

/// One decoded Property Entry (MS-OXMSG 2.4.2). `tail` is the raw final 8
/// bytes: for a fixed-length entry this is the value itself (never
/// interpreted here); for a variable-length entry it is Size (4 bytes) then
/// Reserved (4 bytes).
struct DecodedPropertyEntry {
    property_type: u16,
    property_id: u16,
    flags: u32,
    tail: [u8; 8],
}

struct DecodedPropertiesStream {
    entries: Vec<DecodedPropertyEntry>,
    /// Bytes remaining after the last full 16-byte entry. Always 0 for a
    /// well-formed stream.
    trailing_bytes: usize,
}

/// Parses the entry array of a `__properties_version1.0` stream. Returns
/// `None` only when `bytes` is shorter than `header_len`, which the caller
/// reports as an anomaly rather than silently skipping.
fn decode_properties_stream(bytes: &[u8], header_len: usize) -> Option<DecodedPropertiesStream> {
    let body = bytes.get(header_len..)?;
    let mut entries = Vec::with_capacity(body.len() / 16);
    let mut offset = 0;
    while offset + 16 <= body.len() {
        let property_type = u16::from_le_bytes([body[offset], body[offset + 1]]);
        let property_id = u16::from_le_bytes([body[offset + 2], body[offset + 3]]);
        let flags = u32::from_le_bytes([
            body[offset + 4],
            body[offset + 5],
            body[offset + 6],
            body[offset + 7],
        ]);
        let mut tail = [0u8; 8];
        tail.copy_from_slice(&body[offset + 8..offset + 16]);
        entries.push(DecodedPropertyEntry {
            property_type,
            property_id,
            flags,
            tail,
        });
        offset += 16;
    }
    Some(DecodedPropertiesStream {
        entries,
        trailing_bytes: body.len() - offset,
    })
}

// --- Fixed-value structural checks (never print the value itself) --------

/// PT_BOOLEAN's value occupies the first 2 bytes of the entry's value field
/// (MS-OXCDATA 2.11.1); the only defined encodings are 0x0000 and 0x0001.
fn is_valid_boolean_encoding(tail: &[u8; 8]) -> bool {
    tail[1] == 0 && matches!(tail[0], 0 | 1)
}

/// A property value that fits inline in a Property Entry's 8-byte value
/// field, decoded to its real Rust type (MS-OXCDATA 2.11.1). Kept as a
/// typed, lossless intermediate representation -- not yet formatted for
/// display or written anywhere -- consistent with design principle 3
/// (Markdown is a projection, not the canonical representation).
#[derive(Debug, Clone, Copy, PartialEq)]
enum DecodedFixedValue {
    Short(i16),
    Long(i32),
    Float(f32),
    Double(f64),
    /// PtypCurrency: a signed 64-bit integer scaled by 10000 (four decimal
    /// places). Kept as the raw scaled integer, not divided down to a
    /// float, to avoid any precision loss -- dividing by 10000 for display
    /// is a later, presentation-layer concern.
    Currency(i64),
    /// An OLE Automation date (days since 1899-12-30; the fractional part
    /// is time-of-day). Calendar conversion is deliberately not attempted
    /// here.
    AppTime(f64),
    /// PtypErrorCode: a 32-bit MAPI error/status code.
    Error(u32),
    Boolean(bool),
    I8(i64),
    /// PtypTime: a Windows FILETIME -- 100-nanosecond intervals since
    /// 1601-01-01 UTC. Kept as raw ticks; calendar conversion is
    /// deliberately not attempted here.
    SysTime(u64),
}

/// Decodes a fixed-length entry's 8-byte value field into its real type.
/// `base_type` must be one [`is_fixed_length_base_type`] accepts; anything
/// else returns `None` rather than guessing.
fn decode_fixed_value(base_type: u16, tail: &[u8; 8]) -> Option<DecodedFixedValue> {
    match base_type {
        0x0002 => Some(DecodedFixedValue::Short(i16::from_le_bytes([
            tail[0], tail[1],
        ]))),
        0x0003 => Some(DecodedFixedValue::Long(i32::from_le_bytes([
            tail[0], tail[1], tail[2], tail[3],
        ]))),
        0x0004 => Some(DecodedFixedValue::Float(f32::from_le_bytes([
            tail[0], tail[1], tail[2], tail[3],
        ]))),
        0x0005 => Some(DecodedFixedValue::Double(f64::from_le_bytes(*tail))),
        0x0006 => Some(DecodedFixedValue::Currency(i64::from_le_bytes(*tail))),
        0x0007 => Some(DecodedFixedValue::AppTime(f64::from_le_bytes(*tail))),
        0x000A => Some(DecodedFixedValue::Error(u32::from_le_bytes([
            tail[0], tail[1], tail[2], tail[3],
        ]))),
        0x000B => Some(DecodedFixedValue::Boolean(tail[0] != 0)),
        0x0014 => Some(DecodedFixedValue::I8(i64::from_le_bytes(*tail))),
        0x0040 => Some(DecodedFixedValue::SysTime(u64::from_le_bytes(*tail))),
        _ => None,
    }
}

/// Decodes PT_UNICODE (PtypString) bytes as UTF-16LE. The stream itself
/// does not include a null terminator -- MS-OXMSG's declared Size field
/// accounts for one that isn't actually present in the stream (confirmed
/// against the corpus via `expected_size_field_value`), so no terminator
/// handling is needed here. Caller is expected to have already confirmed
/// an even byte length.
fn decode_unicode_value(bytes: &[u8]) -> Result<String, std::string::FromUtf16Error> {
    let code_units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&code_units)
}

/// Windows-1252, per the WHATWG Encoding Standard's windows-1252 index --
/// identical to ISO-8859-1/Latin-1 outside 0x80-0x9F. UNVERIFIED against
/// real fixture data: the 29-file corpus has no PT_STRING8 property at
/// all to check this against, and `PidTagMessageCodepage` isn't consulted
/// here -- this is the conventional default, not a codepage-aware decode.
fn cp1252_to_char(byte: u8) -> Option<char> {
    // Index 0 = 0x80. A 0 entry marks one of the five byte values
    // Windows-1252 leaves genuinely undefined (0x81, 0x8D, 0x8F, 0x90, 0x9D).
    const UPPER: [u16; 32] = [
        0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x0000, 0x017D, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x0000, 0x017E, 0x0178,
    ];
    if (0x80..=0x9F).contains(&byte) {
        match UPPER[(byte - 0x80) as usize] {
            0x0000 => None,
            code => char::from_u32(code as u32),
        }
    } else {
        Some(byte as char) // identical to Latin-1 for every other byte
    }
}

/// Decodes PT_STRING8 bytes as Windows-1252, replacing any of the five
/// undefined byte values with U+FFFD and reporting how many were replaced
/// -- the same loss-is-explicit pattern as `String::from_utf8_lossy`.
fn decode_string8_cp1252(bytes: &[u8]) -> (String, u32) {
    let mut s = String::with_capacity(bytes.len());
    let mut undefined = 0u32;
    for &b in bytes {
        match cp1252_to_char(b) {
            Some(c) => s.push(c),
            None => {
                undefined += 1;
                s.push('\u{FFFD}');
            }
        }
    }
    (s, undefined)
}

// --- Variable-length value stream cross-check (never read as content) ----

fn expected_variable_stream_path(parent: &Path, property_id: u16, property_type: u16) -> PathBuf {
    parent.join(format!("__substg1.0_{property_id:04X}{property_type:04X}"))
}

/// MS-OXMSG 2.4.2.2: the declared Size field equals the value stream's byte
/// length for most types, +2 for PT_UNICODE, +1 for PT_STRING8.
fn expected_size_field_value(property_type: u16, actual_stream_len: u64) -> u64 {
    match property_type {
        0x001F => actual_stream_len + 2, // PtypString / PT_UNICODE
        0x001E => actual_stream_len + 1, // PtypString8
        _ => actual_stream_len,
    }
}

// --- Named-property resolution (MS-OXMSG 2.2.3) ---------------------------

// Well-known property-set GUIDs, MS-OXPROPS 1.3.2 (little-endian byte
// order). A deliberately small set for this slice; anything else is
// reported as "custom" -- never by its raw GUID bytes.
const PSETID_ADDRESS: [u8; 16] = [
    0x04, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const PSETID_APPOINTMENT: [u8; 16] = [
    0x02, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const PSETID_COMMON: [u8; 16] = [
    0x08, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const PSETID_LOG: [u8; 16] = [
    0x0A, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const PSETID_NOTE: [u8; 16] = [
    0x0E, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const PSETID_TASK: [u8; 16] = [
    0x03, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
/// {00020386-0000-0000-C000-000000000046}, MS-OXPROPS 1.3.2 -- named
/// properties synthesized from MIME/internet-header fields. Confirmed
/// present in real fixture data (file 0's GUID stream) during the
/// bit-layout investigation; added now that resolution is fixed.
const PS_INTERNET_HEADERS: [u8; 16] = [
    0x86, 0x03, 0x02, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];

fn classify_well_known_property_set(guid: &[u8; 16]) -> Option<&'static str> {
    match *guid {
        PSETID_ADDRESS => Some("PSETID_Address"),
        PSETID_APPOINTMENT => Some("PSETID_Appointment"),
        PSETID_COMMON => Some("PSETID_Common"),
        PSETID_LOG => Some("PSETID_Log"),
        PSETID_NOTE => Some("PSETID_Note"),
        PSETID_TASK => Some("PSETID_Task"),
        PS_INTERNET_HEADERS => Some("PS_INTERNET_HEADERS"),
        _ => None,
    }
}

/// A decoded Entry Stream record (MS-OXMSG 2.2.3.2.4). `name_id_or_offset`
/// is either a numeric LID (a small application-defined integer -- not
/// content) or a byte offset into the string stream (never followed by
/// this diagnostic).
///
/// Bit layout, confirmed against real fixture bytes (see
/// docs/verification/oxmsg-results.md) rather than assumed from the spec
/// text or a partially-read crate source, both of which turned out wrong
/// on this point: the HIGH 16 bits of the second u32 are Property Index
/// (matches the entry's own array position exactly, MS-OXMSG 2.2.3.2.4).
/// The LOW 16 bits pack GUID Index and Property Kind together, with Kind
/// as the low-order bit and GUID Index in the bits above it -- not GUID
/// Index in the low 15 bits with Kind as the top bit, and not in the high
/// 16 bits at all.
struct NamedPropertyEntryRaw {
    name_id_or_offset: u32,
    guid_index: u16,
    is_string: bool,
    /// The entry's own claimed array position. MS-OXMSG requires this to
    /// equal the entry's actual offset in the stream; kept so callers can
    /// cross-check it against the position they looked it up by.
    property_index: u16,
}

fn decode_named_property_entry(bytes: &[u8; 8]) -> NamedPropertyEntryRaw {
    let name_id_or_offset = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let index_kind = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let guid_and_kind = (index_kind & 0xFFFF) as u16;
    let property_index = (index_kind >> 16) as u16;
    NamedPropertyEntryRaw {
        name_id_or_offset,
        guid_index: guid_and_kind >> 1,
        is_string: guid_and_kind & 1 != 0,
        property_index,
    }
}

enum NamedPropertySet {
    PsMapi,
    PsPublicStrings,
    WellKnown(&'static str),
    Custom,
    /// `guid_index` pointed outside the GUID stream -- a real anomaly.
    OutOfRange,
}

struct NamedPropertyMap {
    guid_stream: Vec<u8>,
    entry_stream: Vec<u8>,
}

impl NamedPropertyMap {
    fn lookup(&self, property_id: u16) -> Option<NamedPropertyEntryRaw> {
        let index = property_id.checked_sub(0x8000)? as usize;
        let offset = index * 8;
        let bytes = self.entry_stream.get(offset..offset + 8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(bytes);
        Some(decode_named_property_entry(&arr))
    }

    fn resolve_set(&self, guid_index: u16) -> NamedPropertySet {
        match guid_index {
            1 => NamedPropertySet::PsMapi,
            2 => NamedPropertySet::PsPublicStrings,
            n if n >= 3 => {
                let offset = (n - 3) as usize * 16;
                match self.guid_stream.get(offset..offset + 16) {
                    Some(bytes) => {
                        let mut g = [0u8; 16];
                        g.copy_from_slice(bytes);
                        match classify_well_known_property_set(&g) {
                            Some(name) => NamedPropertySet::WellKnown(name),
                            None => NamedPropertySet::Custom,
                        }
                    }
                    None => NamedPropertySet::OutOfRange,
                }
            }
            _ => NamedPropertySet::OutOfRange, // 0 is not a defined guid index
        }
    }
}

/// Reads and parses the named property mapping storage, if present. Per
/// MS-OXMSG 2.2.3, this always lives at the top level, even for named
/// properties on an embedded message (Embedded Message objects MUST NOT
/// have their own).
fn read_named_property_map(
    comp: &mut cfb::CompoundFile<std::fs::File>,
) -> Option<NamedPropertyMap> {
    let guid_stream =
        read_stream_bytes(comp, Path::new("/__nameid_version1.0/__substg1.0_00020102"))?;
    let entry_stream =
        read_stream_bytes(comp, Path::new("/__nameid_version1.0/__substg1.0_00030102"))?;
    Some(NamedPropertyMap {
        guid_stream,
        entry_stream,
    })
}

/// Reads a stream's full contents by path. `comp` must be the same open
/// container the path came from.
fn read_stream_bytes(comp: &mut cfb::CompoundFile<std::fs::File>, path: &Path) -> Option<Vec<u8>> {
    let mut stream = comp.open_stream(path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn cfb_entry_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

/// Everything `inspect_oxmsg` needs from a CFB entry, captured up front so
/// the immutable borrow from `comp.walk()` ends before the second pass needs
/// `&mut comp` to read stream contents.
struct CollectedOxmsgEntry {
    path: PathBuf,
    is_root: bool,
    is_stream: bool,
    len: u64,
    clsid: String,
}

fn inspect_oxmsg(comp: &mut cfb::CompoundFile<std::fs::File>, totals: &mut OxmsgTotals) {
    let mut saw_properties_stream = false;
    let message_shaped_parents = message_shaped_parent_paths(&*comp);

    // Pass 1: classify every entry from its name and position. This only
    // needs immutable access, so results are collected up front, ending
    // that borrow before pass 2 needs `&mut comp` to read stream contents.
    let entries: Vec<CollectedOxmsgEntry> = comp
        .walk()
        .map(|e| CollectedOxmsgEntry {
            path: e.path().to_path_buf(),
            is_root: e.is_root(),
            is_stream: e.is_stream(),
            len: e.len(),
            clsid: e.clsid().to_string(),
        })
        .collect();

    for entry in &entries {
        totals.total_entries += 1;
        let object_kind = cfb_object_kind(entry.is_stream);

        // Ancestry outranks name: anything beneath a custom attachment
        // storage is application-defined, whatever it happens to be called.
        if let Some(payload_root) =
            enclosing_custom_payload_root(&entry.path, &message_shaped_parents)
        {
            totals.opaque_payload_entries_total += 1;
            let below = cfb_entry_depth(&entry.path).saturating_sub(cfb_entry_depth(&payload_root));
            *totals
                .opaque_payload_entries
                .entry((object_kind, below))
                .or_insert(0) += 1;
            continue;
        }

        let entry_kind = classify_oxmsg_entry(&cfb_entry_name(&entry.path), entry.is_root);
        if let Some(mismatch) = recognized_name_type_mismatch(&entry_kind, object_kind) {
            *totals
                .recognized_name_type_mismatches
                .entry(mismatch)
                .or_insert(0) += 1;
        }
        match entry_kind {
            OxmsgEntryKind::Root => {
                totals.root_entries_total += 1;
                totals.recognized_entries_total += 1;
            }
            OxmsgEntryKind::PropertiesStream => {
                totals.recognized_entries_total += 1;
                totals.properties_stream_entries_total += 1;
                saw_properties_stream = true;
                totals.properties_stream_bytes_total += entry.len;

                let scope = oxmsg_entry_scope(&entry.path);
                *totals.properties_streams_by_scope.entry(scope).or_insert(0) += 1;
            }
            OxmsgEntryKind::PropertyStream { prop_id, indexed } => {
                totals.recognized_entries_total += 1;
                totals.property_streams_total += 1;
                if indexed {
                    totals.indexed_property_streams_total += 1;
                }

                let scope = oxmsg_entry_scope(&entry.path);
                *totals.property_streams_by_scope.entry(scope).or_insert(0) += 1;
                *totals
                    .property_id_counts_by_scope
                    .entry((scope, prop_id))
                    .or_insert(0) += 1;
                // Named-property storage streams are not MAPI properties:
                // their four-hex prefix is a stream identifier (0x0002-0x0004
                // and the 0x1000-range hash buckets), which would collide
                // with real property IDs such as PidTagBody (0x1000).
                if scope != OxmsgEntryScope::NamedPropertyStorage {
                    *totals.property_id_counts.entry(prop_id).or_insert(0) += 1;
                }
            }
            OxmsgEntryKind::AttachmentStorage => {
                totals.recognized_entries_total += 1;
                totals.attachment_storages_total += 1;
            }
            OxmsgEntryKind::RecipientStorage => {
                totals.recognized_entries_total += 1;
                totals.recipient_storages_total += 1;
            }
            OxmsgEntryKind::NamedPropertyStorage => {
                totals.recognized_entries_total += 1;
                totals.named_property_storages_total += 1;
            }
            OxmsgEntryKind::EmbeddedObjectStorage => {
                totals.recognized_entries_total += 1;
                totals.embedded_object_storages_total += 1;
                let shape = if message_shaped_parents.contains(&entry.path) {
                    totals.embedded_object_storages_message_shaped_total += 1;
                    "message_shaped"
                } else {
                    totals.embedded_object_storages_custom_total += 1;
                    "custom"
                };
                *totals
                    .embedded_object_storages_by_shape
                    .entry((shape, entry.clsid.clone()))
                    .or_insert(0) += 1;
            }
            OxmsgEntryKind::Unrecognized => {
                totals.unrecognized_entries_total += 1;
                *totals
                    .unrecognized_entries
                    .entry((
                        object_kind,
                        cfb_entry_depth(&entry.path),
                        unrecognized_name_shape(&cfb_entry_name(&entry.path)),
                        oxmsg_ancestry_shape(&entry.path),
                    ))
                    .or_insert(0) += 1;
            }
        }
    }

    if saw_properties_stream {
        totals.has_properties_stream += 1;
    }
    // Named properties are resolved once per file (the mapping storage is
    // shared by the whole message, embedded messages included) rather than
    // once per entry.
    let named_property_map = read_named_property_map(comp);

    // Pass 2: decode the entry array of every properties stream found
    // above. This reads stream contents, but reports only structural
    // fields (type, ID, flags, and variable-length size/reserved) -- never
    // a property's value.
    for entry in &entries {
        if !entry.is_stream || cfb_entry_name(&entry.path) != "__properties_version1.0" {
            continue;
        }
        let scope = oxmsg_entry_scope(&entry.path);
        let Some(header_len) = properties_stream_header_len(scope) else {
            totals.properties_stream_unexpected_scope_total += 1;
            continue;
        };
        let Some(bytes) = read_stream_bytes(comp, &entry.path) else {
            totals.properties_stream_read_errors += 1;
            continue;
        };
        let Some(decoded) = decode_properties_stream(&bytes, header_len) else {
            totals.properties_stream_too_short_for_header_total += 1;
            continue;
        };
        if decoded.trailing_bytes != 0 {
            totals.properties_stream_trailing_bytes_total += 1;
        }
        for prop_entry in decoded.entries {
            totals.properties_entries_total += 1;
            *totals
                .property_type_counts_by_scope
                .entry((scope, prop_entry.property_type))
                .or_insert(0) += 1;
            *totals
                .property_entry_flags_counts
                .entry(prop_entry.flags)
                .or_insert(0) += 1;

            match classify_property_entry_shape(prop_entry.property_type) {
                PropertyEntryShape::FixedInline => {
                    totals.properties_entries_fixed_inline_total += 1;
                    if prop_entry.property_type == 0x000B
                        && !is_valid_boolean_encoding(&prop_entry.tail)
                    {
                        totals.fixed_boolean_invalid_encoding_total += 1;
                    }
                    if let Some(value) =
                        decode_fixed_value(prop_entry.property_type, &prop_entry.tail)
                    {
                        let non_finite = match value {
                            DecodedFixedValue::Float(f) => !f.is_finite(),
                            DecodedFixedValue::Double(d) | DecodedFixedValue::AppTime(d) => {
                                !d.is_finite()
                            }
                            _ => false,
                        };
                        if non_finite {
                            totals.fixed_float_non_finite_total += 1;
                        }
                    }
                }
                PropertyEntryShape::VariableSingle => {
                    totals.properties_entries_variable_single_total += 1;
                    // PT_OBJECT (0x000D) properties point at a storage, not
                    // a stream -- already covered by the embedded-object
                    // accounting above.
                    let declared_size = u32::from_le_bytes([
                        prop_entry.tail[0],
                        prop_entry.tail[1],
                        prop_entry.tail[2],
                        prop_entry.tail[3],
                    ]);
                    if prop_entry.property_type != 0x000D && declared_size != 0xFFFF_FFFF {
                        let parent = entry.path.parent().unwrap_or(Path::new("/"));
                        let value_path = expected_variable_stream_path(
                            parent,
                            prop_entry.property_id,
                            prop_entry.property_type,
                        );
                        match read_stream_bytes(comp, &value_path) {
                            None => totals.variable_value_stream_missing_total += 1,
                            Some(value_bytes) => {
                                totals.variable_value_stream_found_total += 1;
                                let expected = expected_size_field_value(
                                    prop_entry.property_type,
                                    value_bytes.len() as u64,
                                );
                                if expected != declared_size as u64 {
                                    totals.variable_value_size_mismatch_total += 1;
                                }
                                match prop_entry.property_type {
                                    0x001F => {
                                        if value_bytes.len() % 2 != 0 {
                                            totals.variable_value_odd_utf16_length_total += 1;
                                        } else if decode_unicode_value(&value_bytes).is_err() {
                                            totals.variable_unicode_decode_errors_total += 1;
                                        }
                                    }
                                    0x001E => {
                                        let (_, undefined_count) =
                                            decode_string8_cp1252(&value_bytes);
                                        totals.variable_string8_undefined_byte_total +=
                                            undefined_count as u64;
                                    }
                                    0x0048 => {
                                        if value_bytes.len() != 16 {
                                            totals.variable_clsid_wrong_length_total += 1;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                PropertyEntryShape::VariableMultivalued => {
                    totals.properties_entries_variable_multivalued_total += 1;
                }
            }

            // PidTagAttachDataObject (0x3701, PT_OBJECT 0x000D) on an
            // attachment: an independent, property-level cross-check of the
            // CFB-structural message-shaped/custom classification
            // (MS-OXMSG 2.4.2.2).
            if scope == OxmsgEntryScope::Attachment
                && prop_entry.property_id == 0x3701
                && prop_entry.property_type == 0x000D
            {
                let tail = prop_entry.tail;
                let size = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
                let reserved = u32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]);
                *totals
                    .attach_data_object_reserved_counts
                    .entry(reserved)
                    .or_insert(0) += 1;
                if size != 0xFFFF_FFFF {
                    totals.attach_data_object_size_sentinel_mismatches += 1;
                }
            }

            // Named-property resolution (MS-OXMSG 2.2.3): identity only
            // (property set + numeric-or-string), never a string name.
            if prop_entry.property_id >= 0x8000 {
                totals.named_properties_seen_total += 1;
                match &named_property_map {
                    None => totals.named_properties_map_missing_total += 1,
                    Some(map) => match map.lookup(prop_entry.property_id) {
                        None => totals.named_properties_unresolvable_total += 1,
                        Some(raw) => {
                            let expected_index = prop_entry.property_id - 0x8000;
                            if raw.property_index != expected_index {
                                totals.named_properties_index_mismatch_total += 1;
                            }
                            if raw.is_string {
                                totals.named_properties_string_kind_total += 1;
                            } else {
                                totals.named_properties_numeric_kind_total += 1;
                                *totals
                                    .named_property_numeric_lids
                                    .entry(raw.name_id_or_offset)
                                    .or_insert(0) += 1;
                            }
                            match map.resolve_set(raw.guid_index) {
                                NamedPropertySet::PsMapi => {
                                    *totals.named_property_sets.entry("PS_MAPI").or_insert(0) += 1;
                                }
                                NamedPropertySet::PsPublicStrings => {
                                    *totals
                                        .named_property_sets
                                        .entry("PS_PUBLIC_STRINGS")
                                        .or_insert(0) += 1;
                                }
                                NamedPropertySet::WellKnown(name) => {
                                    *totals.named_property_sets.entry(name).or_insert(0) += 1;
                                }
                                NamedPropertySet::Custom => {
                                    *totals.named_property_sets.entry("custom").or_insert(0) += 1;
                                }
                                NamedPropertySet::OutOfRange => {
                                    totals.named_properties_guid_out_of_range_total += 1;
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum OxmsgEntryKind {
    Root,
    PropertiesStream,
    PropertyStream { prop_id: u16, indexed: bool },
    AttachmentStorage,
    RecipientStorage,
    NamedPropertyStorage,
    EmbeddedObjectStorage,
    Unrecognized,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum OxmsgEntryScope {
    Message,
    Recipient,
    Attachment,
    EmbeddedObject,
    NamedPropertyStorage,
}

impl OxmsgEntryScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Recipient => "recipient",
            Self::Attachment => "attachment",
            Self::EmbeddedObject => "embedded_object",
            Self::NamedPropertyStorage => "named_property_storage",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CfbObjectKind {
    Storage,
    Stream,
}

impl CfbObjectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Stream => "stream",
        }
    }
}

fn cfb_object_kind(is_stream: bool) -> CfbObjectKind {
    if is_stream {
        CfbObjectKind::Stream
    } else {
        CfbObjectKind::Storage
    }
}

fn cfb_entry_depth(path: &Path) -> u64 {
    path.components().count().saturating_sub(1) as u64
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum UnrecognizedNameShape {
    MalformedPropertyStream,
    OtherReserved,
    Other,
}

impl UnrecognizedNameShape {
    fn as_str(self) -> &'static str {
        match self {
            Self::MalformedPropertyStream => "malformed_property_stream_name",
            Self::OtherReserved => "other_reserved_name",
            Self::Other => "other_name",
        }
    }
}

fn unrecognized_name_shape(name: &str) -> UnrecognizedNameShape {
    if name.starts_with("__substg1.0_") {
        UnrecognizedNameShape::MalformedPropertyStream
    } else if name.starts_with("__") {
        UnrecognizedNameShape::OtherReserved
    } else {
        UnrecognizedNameShape::Other
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RecognizedNameTypeMismatch {
    StorageNameIsStream,
    StreamNameIsStorage,
}

impl RecognizedNameTypeMismatch {
    fn as_str(self) -> &'static str {
        match self {
            Self::StorageNameIsStream => "storage_name_is_stream",
            Self::StreamNameIsStorage => "stream_name_is_storage",
        }
    }
}

fn recognized_name_type_mismatch(
    entry_kind: &OxmsgEntryKind,
    object_kind: CfbObjectKind,
) -> Option<RecognizedNameTypeMismatch> {
    match (entry_kind, object_kind) {
        (
            OxmsgEntryKind::PropertiesStream | OxmsgEntryKind::PropertyStream { .. },
            CfbObjectKind::Storage,
        ) => Some(RecognizedNameTypeMismatch::StreamNameIsStorage),
        (
            OxmsgEntryKind::AttachmentStorage
            | OxmsgEntryKind::RecipientStorage
            | OxmsgEntryKind::NamedPropertyStorage
            | OxmsgEntryKind::EmbeddedObjectStorage,
            CfbObjectKind::Stream,
        ) => Some(RecognizedNameTypeMismatch::StorageNameIsStream),
        _ => None,
    }
}

/// Classifies a single CFB entry by name alone, using the MS-CFB
/// storage/stream naming conventions MS-OXMSG defines (see the module
/// comment above). Takes a plain `&str` rather than a `cfb::Entry`
/// directly -- that type has no public constructor, so keeping the
/// classification logic pure and string-based is what makes it possible
/// to unit-test without a real CFB file on disk.
fn classify_oxmsg_entry(name: &str, is_root: bool) -> OxmsgEntryKind {
    if is_root {
        return OxmsgEntryKind::Root;
    }
    if name == "__properties_version1.0" {
        return OxmsgEntryKind::PropertiesStream;
    }
    if name == "__nameid_version1.0" {
        return OxmsgEntryKind::NamedPropertyStorage;
    }
    if name.starts_with("__attach_version1.0_#") {
        return OxmsgEntryKind::AttachmentStorage;
    }
    if name.starts_with("__recip_version1.0_#") {
        return OxmsgEntryKind::RecipientStorage;
    }
    if name == "__substg1.0_3701000D" {
        return OxmsgEntryKind::EmbeddedObjectStorage;
    }
    if let Some((prop_id, indexed)) = parse_property_stream_name(name) {
        return OxmsgEntryKind::PropertyStream { prop_id, indexed };
    }
    OxmsgEntryKind::Unrecognized
}

fn parse_property_stream_name(name: &str) -> Option<(u16, bool)> {
    let suffix = name.strip_prefix("__substg1.0_")?;

    if suffix.len() == 8 {
        let prop_id = u16::from_str_radix(&suffix[0..4], 16).ok()?;
        u16::from_str_radix(&suffix[4..8], 16).ok()?;
        return Some((prop_id, false));
    }

    let (tag, index) = suffix.split_once('-')?;
    if tag.len() != 8 || index.len() != 8 {
        return None;
    }

    let prop_id = u16::from_str_radix(&tag[0..4], 16).ok()?;
    u16::from_str_radix(&tag[4..8], 16).ok()?;
    u32::from_str_radix(index, 16).ok()?;

    Some((prop_id, true))
}

fn oxmsg_entry_scope(path: &Path) -> OxmsgEntryScope {
    let mut scope = OxmsgEntryScope::Message;

    for component in path.components() {
        let Some(name) = component.as_os_str().to_str() else {
            continue;
        };

        match name {
            "__nameid_version1.0" => {
                scope = OxmsgEntryScope::NamedPropertyStorage;
            }
            "__substg1.0_3701000D" => {
                scope = OxmsgEntryScope::EmbeddedObject;
            }
            name if name.starts_with("__attach_version1.0_#") => {
                scope = OxmsgEntryScope::Attachment;
            }
            name if name.starts_with("__recip_version1.0_#") => {
                scope = OxmsgEntryScope::Recipient;
            }
            _ => {}
        }
    }

    scope
}

const EMBEDDED_OBJECT_STORAGE_NAME: &str = "__substg1.0_3701000D";

/// Parents of every `__properties_version1.0` stream. A `3701000D` storage in
/// this set is message-shaped (an embedded message); one not in it is a
/// custom attachment storage.
fn message_shaped_parent_paths(comp: &cfb::CompoundFile<std::fs::File>) -> BTreeSet<PathBuf> {
    comp.walk()
        .filter(|e| e.is_stream() && e.name() == "__properties_version1.0")
        .filter_map(|e| e.path().parent().map(Path::to_path_buf))
        .collect()
}

/// If `path` lies beneath a custom (non-message-shaped) embedded-object
/// storage, returns the outermost such storage's path. The storage itself is
/// not "beneath" itself, so it keeps its own classification.
fn enclosing_custom_payload_root(
    path: &Path,
    message_shaped: &BTreeSet<PathBuf>,
) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .filter(|a| a.file_name().and_then(|n| n.to_str()) == Some(EMBEDDED_OBJECT_STORAGE_NAME))
        .filter(|a| !message_shaped.contains(*a))
        .last()
        .map(Path::to_path_buf)
}

/// Privacy-safe ancestry: fixed-vocabulary tokens only, never entry names.
fn oxmsg_ancestry_shape(path: &Path) -> String {
    let mut tokens: Vec<&'static str> = Vec::new();
    let ancestors: Vec<&Path> = path.ancestors().skip(1).collect();
    for ancestor in ancestors.iter().rev() {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue; // the root has no file name
        };
        tokens.push(match name {
            "__nameid_version1.0" => "named_property_storage",
            EMBEDDED_OBJECT_STORAGE_NAME => "embedded_object",
            n if n.starts_with("__attach_version1.0_#") => "attachment",
            n if n.starts_with("__recip_version1.0_#") => "recipient",
            _ => "other_storage",
        });
    }
    if tokens.is_empty() {
        "root".to_string()
    } else {
        format!("root/{}", tokens.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Custom MS-OXMSG parser groundwork (--oxmsg) -------------------------

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
            recognized_name_type_mismatch(
                &OxmsgEntryKind::AttachmentStorage,
                CfbObjectKind::Stream
            ),
            Some(RecognizedNameTypeMismatch::StorageNameIsStream)
        );
        assert_eq!(
            recognized_name_type_mismatch(
                &OxmsgEntryKind::RecipientStorage,
                CfbObjectKind::Storage
            ),
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
}
