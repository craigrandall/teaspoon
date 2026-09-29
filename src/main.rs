use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
type CompoundFile = cfb::CompoundFile<std::fs::File>;

use msg_parser::Outlook;
use outlook_pst::{
    ltp::{
        prop_context::PropertyValue,
        table_context::{TableContext, TableContextInfo, TableRowColumnValue},
    },
    messaging::{folder::Folder as PstFolder, message::Message as PstMessage, store::Store},
    ndb::node_id::NodeId,
};

// =============================================================================
// CLI and input classification
// =============================================================================

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe inventory for the teaspoon Outlook message miner (PST or MSG)"
)]
struct Args {
    /// A .pst file, a single .msg file, or a directory of .msg files to inspect.
    input: PathBuf,

    /// Structural diagnostic of the custom MS-OXMSG parser (raw CFB
    /// enumeration via the `cfb` crate) instead of the default extraction
    /// report. PST input is unaffected. Retired in M3g.
    #[arg(long)]
    oxmsg: bool,

    /// Compare the custom MS-OXMSG extraction path against `msg_parser`
    /// for the same .msg input. Reads real property content internally to
    /// do the comparison, but prints only match/mismatch counts -- never
    /// the values compared. Also runs the custom path's structural
    /// accounting (every CFB entry classified, every properties stream
    /// and value stream decoded) and prints its gate counters plus
    /// `structural_gate_violations` (0 on a clean corpus). Takes
    /// precedence over --oxmsg and --extract. PST input is unaffected.
    #[arg(long)]
    verify: bool,

    /// Run the custom MS-OXMSG extraction path. Since M3f this is the
    /// default for .msg input, so the flag is a redundant alias kept until
    /// M3g. Zero bytes are counted only for confirmed-empty
    /// PidTagAttachDataBinary streams; an unreadable data stream is
    /// reported separately via attachments_data_stream_missing. Takes
    /// precedence over --oxmsg. PST input is unaffected.
    #[arg(long)]
    extract: bool,
}

enum InputKind {
    Pst,
    Msg {
        files: Vec<PathBuf>,
        /// Subdirectories found directly inside the scanned directory but
        /// not descended into (the scan is deliberately non-recursive).
        /// Surfaced in diagnostic output so this scoping choice is visible
        /// in the output, not just in the source.
        subdirectories_skipped: u64,
    },
}

/// Classifies the input by extension (or, for a directory, by scanning for
/// `.msg` files directly inside it -- not recursive). Never includes the
/// input path itself in any error message: paths can disclose information
/// about the user or their mailbox.
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
            // M3f: the custom MS-OXMSG extraction path is the default for
            // .msg input. `msg_parser` survives only as the independent
            // oracle behind --verify (ADR: custom MS-OXMSG parser graduates
            // to the production MSG path). --extract is now redundant with
            // the default and is kept as an accepted alias until M3g.
            if args.verify {
                run_msg_verify(&files, subdirectories_skipped)
            } else if args.oxmsg && !args.extract {
                run_oxmsg_diagnostic(&files, subdirectories_skipped)
            } else {
                run_msg_extract(&files, subdirectories_skipped)
            }
        }
    }
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
const FROMHTML_MARKER: &[u8] = b"\\fromhtml1";

/// Returns whether decompressed RTF bytes contain the FROMHTML control
/// word. A plain byte search, not a UTF-8/String conversion: RTF is an
/// ASCII-based control-word format (non-ASCII text is escaped as `\'XX`
/// hex sequences), so searching raw bytes avoids encoding questions.
fn rtf_bytes_contain_fromhtml(rtf_bytes: &[u8]) -> bool {
    rtf_bytes
        .windows(FROMHTML_MARKER.len())
        .any(|window| window == FROMHTML_MARKER)
}

/// The result of checking compressed RTF bytes for MS-OXRTFEX HTML
/// encapsulation. Distinct variants (rather than a single bool) keep the
/// common unremarkable cases separate from genuine anomalies, consistent
/// with the project's no-silent-loss principle.
enum RtfHtmlCheck {
    NoRtfProperty,
    NotBinary,
    DecompressionFailed,
    Decompressed {
        contains_fromhtml: bool,
        decompressed_bytes: usize,
    },
}

/// Checks raw `PidTagRtfCompressed` bytes for MS-OXRTFEX HTML
/// encapsulation (see [`FROMHTML_MARKER`]). Decompression uses
/// `compressed-rtf` (MS-OXRTFCP); its magic numbers and dictionary were
/// independently cross-checked against `msg_parser`'s own implementation
/// and match. Never returns or exposes the actual RTF or HTML content.
///
/// `compressed_rtf::decompress_rtf` unconditionally indexes the first 16
/// bytes of its input (its MS-OXRTFCP header read) and panics rather than
/// erroring if given fewer -- confirmed by reading the crate's source, not
/// assumed. The length guard keeps truncated or corrupt data from
/// crashing tsp.
fn check_compressed_rtf_bytes(compressed: &[u8]) -> RtfHtmlCheck {
    if compressed.len() < 16 {
        return RtfHtmlCheck::DecompressionFailed;
    }
    match compressed_rtf::decompress_rtf(compressed) {
        Ok(rtf) => RtfHtmlCheck::Decompressed {
            contains_fromhtml: rtf_bytes_contain_fromhtml(rtf.as_bytes()),
            decompressed_bytes: rtf.len(),
        },
        Err(_) => RtfHtmlCheck::DecompressionFailed,
    }
}

// =============================================================================
// Shared: MAPI property vocabulary (PST, MSG, and custom-OXMSG paths alike)
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
struct BodyCounters {
    plain: u64,
    html: u64,
    html_native: u64,
    html_via_rtf: u64,
    rtf: u64,
    rtf_decompression_errors: u64,
    /// Sum of decompressed-RTF byte lengths across every message where
    /// decompression succeeded (regardless of whether the FROMHTML marker
    /// was found) -- a size-only diagnostic so the PST and MSG sides can be
    /// compared without ever printing content.
    rtf_decompressed_bytes_total: u64,
}

impl BodyCounters {
    /// Records body-*availability* only. Never receives or touches actual
    /// body content -- callers must pass presence booleans.
    fn record(
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

    fn note_decompressed_bytes(&mut self, decompressed_bytes: usize) {
        self.rtf_decompressed_bytes_total += decompressed_bytes as u64;
    }

    fn note_decompression_error(&mut self) {
        self.rtf_decompression_errors += 1;
    }
}

/// The messages-with-any / total / max-on-a-message triple, shared by
/// recipient and attachment counting on both the PST and MSG sides.
#[derive(Default)]
struct CountStats {
    with_any: u64,
    total: u64,
    max: u64,
}

impl CountStats {
    fn record(&mut self, count: u64) {
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
struct ZeroByteStats {
    by_value: u64,
    other_method: u64,
}

impl ZeroByteStats {
    /// `size_is_zero` must already be `false` for a missing or unreadable
    /// size; `is_by_value` should be `false` when the method is missing or
    /// is any non-by-value method.
    fn record(&mut self, size_is_zero: bool, is_by_value: bool) {
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

// =============================================================================
// PST diagnostic (M1)
// =============================================================================

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

    print_pst_report(&totals);

    Ok(())
}

/// Prints the PST inventory report, separated from the walk logic so each
/// reads at one level of abstraction (SLAP). Every key printed here is
/// part of the tool's stable, privacy-safe output vocabulary -- keys are
/// unchanged from previous versions.
fn print_pst_report(totals: &PstTotals) {
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
struct PstTotals {
    folders: u64,
    messages: u64,
    message_open_errors: u64,
    folder_open_errors: u64,
    property_values: u64,

    message_classes: BTreeMap<String, u64>,
    message_class_read_errors: u64,

    bodies: BodyCounters,
    recipients: CountStats,
    attachments: CountStats,

    // Recipient type breakdown.
    recipient_row_read_errors: u64,
    recipients_orig: u64,
    recipients_to: u64,
    recipients_cc: u64,
    recipients_bcc: u64,
    recipients_type_other: u64,
    recipients_type_unknown: u64,

    // Attachment classification.
    attachment_row_read_errors: u64,
    zero_byte_attachments: ZeroByteStats,
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
fn check_rtf_for_encapsulated_html(rtf_property: Option<&PropertyValue>) -> RtfHtmlCheck {
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
fn record_message_class(totals: &mut PstTotals, class: std::io::Result<String>) {
    match class {
        Ok(class) => *totals.message_classes.entry(class).or_insert(0) += 1,
        Err(_) => totals.message_class_read_errors += 1,
    }
}

/// Buckets a single recipient row by its `PidTagRecipientType` value.
/// `None` means the property was missing or not a 32-bit integer on that
/// row.
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
/// `None` means the property was missing or not a 32-bit integer on that
/// row.
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

/// Records presence (not content) of `PidTagAttachContentId`, a common but
/// not definitive signal that an attachment is referenced inline (e.g. an
/// inline image) rather than a standalone file attachment. Zero-size
/// bookkeeping goes through the shared [`ZeroByteStats`] instead.
fn record_attachment_content_id_presence(totals: &mut PstTotals, has_content_id: bool) {
    if has_content_id {
        totals.attachments_with_content_id += 1;
    }
}

/// Locates the index of a column by MAPI property ID within a table's
/// column descriptors, so its value can be looked up per row via
/// [`read_i32_at`] or a presence check.
fn column_index(context: &TableContextInfo, prop_id: u16) -> Option<usize> {
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

fn inspect_attachments(message: &dyn PstMessage, totals: &mut PstTotals) {
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

// =============================================================================
// MSG diagnostic (msg_parser adapter)
// =============================================================================

/// PidTagAttachMethod values as exposed by `msg_parser`'s `attach_method`
/// field. Same MAPI vocabulary as the PST side (MS-OXCMSG), but msg_parser
/// does not currently expose the by-reference variants (2/3/4) separately,
/// so they fall into `attachments_method_other` here if ever encountered.
const MSG_ATTACH_METHOD_BY_VALUE: u32 = 1;
const MSG_ATTACH_METHOD_EMBEDDED_MESSAGE: u32 = 5;
const MSG_ATTACH_METHOD_OLE: u32 = 6;

/// PT_ERROR (0x000A): per MS-OXMSG, a property that is *not set* on an
/// object is still represented in its `__properties_version1.0` entry
/// array as an entry of this type, carrying PidTagNotFound. Presence
/// checks that match on property ID alone would count these
/// placeholders; they must be type-gated out.
const PROP_TYPE_ERROR: u16 = 0x000A;
/// PT_UNSPECIFIED (0x0000): excluded from presence checks for the same
/// reason as [`PROP_TYPE_ERROR`].
const PROP_TYPE_UNSPECIFIED: u16 = 0x0000;

/// Prints the MSG inventory report, separated from the
/// scan loop (SLAP). Keys are unchanged from previous versions.
fn print_msg_report(totals: &MsgTotals) {
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
struct MsgTotals {
    open_errors: u64,

    message_classes: BTreeMap<String, u64>,
    message_class_missing: u64,

    bodies: BodyCounters,

    recipients_to: u64,
    recipients_cc: u64,
    recipients_bcc: u64,
    recipients: CountStats,

    attachments: CountStats,
    zero_byte_attachments: ZeroByteStats,
    attachments_with_content_id: u64,
    attachments_method_by_value: u64,
    attachments_method_embedded_message: u64,
    attachments_method_ole: u64,
    attachments_method_other: u64,

    embedded_messages_opened: u64,
    embedded_message_open_errors: u64,
    embedded_message_classes: BTreeMap<String, u64>,
}

fn record_msg_class(totals: &mut MsgTotals, class: &str) {
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
fn record_msg_recipients(totals: &mut MsgTotals, to: u64, cc: u64, bcc: u64) {
    totals.recipients_to += to;
    totals.recipients_cc += cc;
    totals.recipients_bcc += bcc;
    totals.recipients.record(to + cc + bcc);
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
// Custom MS-OXMSG parser: container naming conventions and entry
// classification (experimental, opt-in via --oxmsg)
// =============================================================================
//
// MS-OXMSG stores every message property inside a CFB (MS-CFB) container as
// one of two things:
// - fixed-length properties, packed together inside a single stream named
//   `__properties_version1.0`;
// - variable-length properties (strings, binary, and variable-length
//   multi-valued values) in streams named `__substg1.0_PPPPTTTT`, where PPPP
//   is the 4-hex-digit property ID and TTTT the 4-hex-digit property type.
//   Variable-length multi-valued values add a zero-based `-NNNNNNNN`
//   value-index suffix.
// Recipients and attachments each get their own numbered sub-storage
// (`__recip_version1.0_#NNNNNNNN`, `__attach_version1.0_#NNNNNNNN`).
// Embedded/custom-object storage is represented by
// `__substg1.0_3701000D`, while named (non-standard) properties get a
// dedicated `__nameid_version1.0` storage. All reporting here is
// name/size/count only -- never content.

/// The CFB storage representing an embedded object (a nested message or a
/// custom/OLE attachment payload), per MS-OXMSG.
const EMBEDDED_OBJECT_STORAGE_NAME: &str = "__substg1.0_3701000D";

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

/// Classifies a single CFB entry by name alone, using the MS-OXMSG
/// storage/stream naming conventions (see the section comment above).
/// Takes a plain `&str` rather than a `cfb::Entry` directly -- that type
/// has no public constructor, so keeping the classification logic pure and
/// string-based is what makes it unit-testable without a real CFB file on
/// disk.
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
    if name == EMBEDDED_OBJECT_STORAGE_NAME {
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
            EMBEDDED_OBJECT_STORAGE_NAME => {
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

/// Parents of every `__properties_version1.0` stream. A `3701000D` storage
/// in this set is message-shaped (an embedded message); one not in it is a
/// custom attachment storage.
fn message_shaped_parent_paths(comp: &CompoundFile) -> BTreeSet<PathBuf> {
    comp.walk()
        .filter(|e| e.is_stream() && e.name() == "__properties_version1.0")
        .filter_map(|e| e.path().parent().map(Path::to_path_buf))
        .collect()
}

/// If `path` lies beneath a custom (non-message-shaped) embedded-object
/// storage, returns the outermost such storage's path. The storage itself
/// is not "beneath" itself, so it keeps its own classification.
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

fn cfb_entry_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

/// Paths of every TOP-LEVEL storage of the given kind. `comp.walk()`
/// traverses the whole tree, including storages nested inside an embedded
/// message's own subtree -- those share the `__recip_version1.0_#*` /
/// `__attach_version1.0_#*` name shape but belong to the inner message.
/// `msg_parser` never opens embedded messages, so its `to`/`cc`/`bcc`/
/// `attachments` only ever reflect the outer message; this filter keeps the
/// custom and msg_parser paths comparing the same scope.
fn top_level_storage_paths(comp: &CompoundFile, kind: OxmsgEntryKind) -> Vec<PathBuf> {
    comp.walk()
        .filter(|e| {
            e.path().parent() == Some(Path::new("/"))
                && matches!(
                    (
                        classify_oxmsg_entry(&cfb_entry_name(e.path()), e.is_root()),
                        kind,
                    ),
                    (
                        OxmsgEntryKind::RecipientStorage,
                        OxmsgEntryKind::RecipientStorage
                    ) | (
                        OxmsgEntryKind::AttachmentStorage,
                        OxmsgEntryKind::AttachmentStorage
                    )
                )
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

// =============================================================================
// Custom MS-OXMSG parser: property-stream and value decoding primitives
//
// Pure, CFB-free decoding helpers. Entry values are never read or reported
// as content -- only structural fields (type, ID, flags, and, for
// variable-length entries, size/reserved).
// =============================================================================

/// Decides only whether a property's value fits inline in a Property
/// Entry's 8-byte value field (MS-OXMSG 2.4.2.1) or lives in a separate
/// stream (2.4.2.2). `base_type` must already have the 0x1000 multi-value
/// bit cleared by the caller.
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
/// interpreted here); for a variable-length entry it is Size (4 bytes)
/// then Reserved (4 bytes).
#[derive(Clone, Copy)]
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

/// PT_BOOLEAN's value occupies the first 2 bytes of the entry's value field
/// (MS-OXCDATA 2.11.1); the only defined encodings are 0x0000 and 0x0001.
fn is_valid_boolean_encoding(tail: &[u8; 8]) -> bool {
    tail[1] == 0 && matches!(tail[0], 0 | 1)
}

/// A property value that fits inline in a Property Entry's 8-byte value
/// field, decoded to its real Rust type (MS-OXCDATA 2.11.1). Kept as a
/// typed, lossless intermediate representation -- not formatted for
/// display or written anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DecodedFixedValue {
    Short(i16),
    Long(i32),
    Float(f32),
    Double(f64),
    /// PtypCurrency: a signed 64-bit integer scaled by 10000. Kept as the
    /// raw scaled integer, not divided down to a float, to avoid precision
    /// loss -- dividing by 10000 is a presentation-layer concern.
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
/// against the corpus via `expected_size_field_value`). Caller is expected
/// to have already confirmed an even byte length.
fn decode_unicode_value(bytes: &[u8]) -> Result<String, std::string::FromUtf16Error> {
    let code_units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16(&code_units)
}

/// Windows-1252, per the WHATWG Encoding Standard's windows-1252 index --
/// identical to ISO-8859-1/Latin-1 outside 0x80-0x9F. UNVERIFIED against
/// real fixture data: the corpus has no PT_STRING8 property to check
/// against, and `PidTagMessageCodepage` isn't consulted here -- this is
/// the conventional default, not a codepage-aware decode.
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
/// undefined byte values with U+FFFD and reporting how many were
/// replaced -- the same loss-is-explicit pattern as
/// `String::from_utf8_lossy`.
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

/// PidTagMessageClass (0x001A): a bounded MAPI vocabulary value.
const PROP_MESSAGE_CLASS: u16 = 0x001A;

/// PidTagMessageCodepage (0x3FFD, PT_LONG): the code page used to encode
/// the non-Unicode (PT_STRING8) string properties of a message object
/// (Microsoft's own property page for it). Zero means "use the folder
/// object's code page", which a standalone `.msg` file cannot supply, so
/// zero is treated as unspecified rather than as a code page.
const PROP_MESSAGE_CODEPAGE: u16 = 0x3FFD;

/// PidTagInternetCodepage (0x3FDE, PT_LONG; PR_INTERNET_CPID): the
/// message's Internet code page. Second link in the chain: consulted only
/// when `PidTagMessageCodepage` is absent or zero. This ordering is a
/// documented teaspoon design choice, not a quotation of a specification
/// rule.
const PROP_INTERNET_CODEPAGE: u16 = 0x3FDE;

/// Last link in the chain: Windows-1252, the conventional default for
/// Western ANSI mail. Reported explicitly (never silently assumed) via
/// [`CodepageSource::Fallback`].
const STRING8_FALLBACK_CODEPAGE: u32 = 1252;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodepageSource {
    MessageCodepage,
    InternetCodepage,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedCodepage {
    codepage: u32,
    source: CodepageSource,
}

/// The PT_STRING8 code page resolution chain: `PidTagMessageCodepage`,
/// then `PidTagInternetCodepage`, then the documented Windows-1252
/// fallback. A value that is absent, zero, or negative is unspecified and
/// falls through to the next link.
fn resolve_string8_codepage(
    message_codepage: Option<i32>,
    internet_codepage: Option<i32>,
) -> ResolvedCodepage {
    let positive = |value: Option<i32>| -> Option<u32> {
        value.and_then(|v| u32::try_from(v).ok()).filter(|&v| v > 0)
    };
    if let Some(codepage) = positive(message_codepage) {
        return ResolvedCodepage {
            codepage,
            source: CodepageSource::MessageCodepage,
        };
    }
    if let Some(codepage) = positive(internet_codepage) {
        return ResolvedCodepage {
            codepage,
            source: CodepageSource::InternetCodepage,
        };
    }
    ResolvedCodepage {
        codepage: STRING8_FALLBACK_CODEPAGE,
        source: CodepageSource::Fallback,
    }
}

/// Reads the first PT_LONG entry with `property_id` from a decoded
/// properties stream, type-gated so a PT_ERROR placeholder never reads as
/// a value.
fn read_long_property(decoded: &DecodedPropertiesStream, property_id: u16) -> Option<i32> {
    decoded.entries.iter().find_map(|entry| {
        if entry.property_id != property_id || entry.property_type != 0x0003 {
            return None;
        }
        match decode_fixed_value(entry.property_type, &entry.tail) {
            Some(DecodedFixedValue::Long(value)) => Some(value),
            _ => None,
        }
    })
}

/// Resolves the PT_STRING8 code page for the message whose own
/// `__properties_version1.0` stream lives directly under `storage`
/// (`/` for the top-level message, the `3701000D` storage for an embedded
/// one). An unreadable properties stream resolves to the fallback rather
/// than failing.
fn extract_string8_codepage(
    comp: &mut CompoundFile,
    storage: &Path,
    header_len: usize,
) -> ResolvedCodepage {
    let decoded = read_stream_bytes(comp, &storage.join("__properties_version1.0"))
        .and_then(|bytes| decode_properties_stream(&bytes, header_len));
    match decoded {
        Some(decoded) => resolve_string8_codepage(
            read_long_property(&decoded, PROP_MESSAGE_CODEPAGE),
            read_long_property(&decoded, PROP_INTERNET_CODEPAGE),
        ),
        None => resolve_string8_codepage(None, None),
    }
}

/// Outcome of decoding PT_STRING8 bytes under a resolved code page. Loss
/// is explicit: a code page this decoder does not implement is reported,
/// never silently decoded as something else.
#[derive(Debug, Clone, PartialEq, Eq)]
enum String8Decoded {
    /// Decoded under an implemented code page; `replaced` counts bytes
    /// with no defined mapping (each became U+FFFD).
    Decoded { text: String, replaced: u32 },
    /// Every byte is 7-bit ASCII and the code page is a known ASCII
    /// superset this decoder does not otherwise implement, so the text is
    /// exact even though the code page's upper half is unsupported.
    AsciiUnderUnsupportedCodepage { text: String },
    /// A code page this decoder does not implement, with bytes that are
    /// not all ASCII. No text is produced.
    UnsupportedCodepage,
}

/// Code pages whose 0x00-0x7F range is plain ASCII, so an all-ASCII byte
/// string decodes exactly under them even without a full decoder. Kept
/// deliberately explicit: EBCDIC and UTF-16/32 code pages are absent.
fn string8_codepage_is_ascii_superset(codepage: u32) -> bool {
    matches!(
        codepage,
        437 | 850 | 852 | 866 | 874 | 932 | 936 | 949 | 950 | 1250
            | 1251 | 1253..=1258 | 20866 | 21866 | 28592..=28599 | 28605
            | 51932 | 51949 | 54936
    )
}

/// Decodes PT_STRING8 bytes under `codepage`. Implemented: 1252, 28591
/// (ISO-8859-1), 20127 (US-ASCII), 65001 (UTF-8). Any other code page
/// decodes only all-ASCII input, and only when it is a known ASCII
/// superset; otherwise it is reported as unsupported.
fn decode_string8_with_codepage(bytes: &[u8], codepage: u32) -> String8Decoded {
    match codepage {
        1252 => {
            let (text, replaced) = decode_string8_cp1252(bytes);
            String8Decoded::Decoded { text, replaced }
        }
        28591 => String8Decoded::Decoded {
            text: bytes.iter().map(|&b| b as char).collect(),
            replaced: 0,
        },
        20127 => {
            let mut text = String::with_capacity(bytes.len());
            let mut replaced = 0u32;
            for &b in bytes {
                if b.is_ascii() {
                    text.push(b as char);
                } else {
                    replaced += 1;
                    text.push('\u{FFFD}');
                }
            }
            String8Decoded::Decoded { text, replaced }
        }
        65001 => match std::str::from_utf8(bytes) {
            Ok(text) => String8Decoded::Decoded {
                text: text.to_string(),
                replaced: 0,
            },
            Err(_) => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                let replaced = text.matches('\u{FFFD}').count() as u32;
                String8Decoded::Decoded { text, replaced }
            }
        },
        other if bytes.is_ascii() && string8_codepage_is_ascii_superset(other) => {
            String8Decoded::AsciiUnderUnsupportedCodepage {
                text: bytes.iter().map(|&b| b as char).collect(),
            }
        }
        _ => String8Decoded::UnsupportedCodepage,
    }
}

/// Decodes one entry of the Named Property String Stream
/// (`__substg1.0_00040102`, MS-OXMSG 2.2.3.1.4): a 4-byte length (the byte
/// count of the UTF-16 string that follows, not including this length
/// prefix or any padding), then the string itself. The decoded string is
/// deliberately treated by callers as present-or-absent only -- this
/// diagnostic never uses the name itself.
fn decode_named_property_string(string_stream: &[u8], offset: u32) -> Option<String> {
    let offset = offset as usize;
    let length_bytes = string_stream.get(offset..offset + 4)?;
    let length = u32::from_le_bytes([
        length_bytes[0],
        length_bytes[1],
        length_bytes[2],
        length_bytes[3],
    ]) as usize;
    let string_bytes = string_stream.get(offset + 4..offset + 4 + length)?;
    decode_unicode_value(string_bytes).ok()
}

fn expected_variable_stream_path(parent: &Path, property_id: u16, property_type: u16) -> PathBuf {
    parent.join(format!("__substg1.0_{property_id:04X}{property_type:04X}"))
}

/// MS-OXMSG 2.4.2.2: the declared Size field equals the value stream's
/// byte length for most types, +2 for PT_UNICODE, +1 for PT_STRING8.
fn expected_size_field_value(property_type: u16, actual_stream_len: u64) -> u64 {
    match property_type {
        0x001F => actual_stream_len + 2, // PtypString / PT_UNICODE
        0x001E => actual_stream_len + 1, // PtypString8
        _ => actual_stream_len,
    }
}

// --- Named-property resolution (MS-OXMSG 2.2.3) ---------------------------

// Well-known property-set GUIDs, MS-OXPROPS 1.3.2 (little-endian byte
// order). A deliberately small set; anything else is reported as
// "custom" -- never by its raw GUID bytes.
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
/// {00020386-0000-0000-C000-000000000046} -- named properties synthesized
/// from MIME/internet-header fields. Confirmed present in real fixture
/// data during the bit-layout investigation.
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
/// Bit layout, confirmed against real fixture bytes rather than assumed
/// from the spec text or a partially-read crate source (both of which
/// turned out wrong on this point): the HIGH 16 bits of the second u32
/// are Property Index (matches the entry's own array position exactly).
/// The LOW 16 bits pack GUID Index and Property Kind together, with Kind
/// as the low-order bit and GUID Index in the bits above it.
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
    string_stream: Vec<u8>,
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

// --- CFB stream reading ----------------------------------------------------

/// Reads a stream's full contents by path. `comp` must be the same open
/// container the path came from.
fn read_stream_bytes(comp: &mut CompoundFile, path: &Path) -> Option<Vec<u8>> {
    let mut stream = comp.open_stream(path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Reads and parses the named property mapping storage, if present. Per
/// MS-OXMSG 2.2.3, this always lives at the top level, even for named
/// properties on an embedded message (Embedded Message objects MUST NOT
/// have their own).
fn read_named_property_map(comp: &mut CompoundFile) -> Option<NamedPropertyMap> {
    let guid_stream =
        read_stream_bytes(comp, Path::new("/__nameid_version1.0/__substg1.0_00020102"))?;
    let entry_stream =
        read_stream_bytes(comp, Path::new("/__nameid_version1.0/__substg1.0_00030102"))?;
    let string_stream =
        read_stream_bytes(comp, Path::new("/__nameid_version1.0/__substg1.0_00040102"))?;
    Some(NamedPropertyMap {
        guid_stream,
        entry_stream,
        string_stream,
    })
}

// =============================================================================
// Custom MS-OXMSG structural diagnostic (--oxmsg): aggregate counters,
// report, and two-pass walk
//
// Pass 1 classifies every CFB entry by name and position (immutable walk);
// pass 2 decodes the entry array of every properties stream found. The
// original single ~320-line `inspect_oxmsg` held three jobs at once; it is
// split so each function does one thing (SLAP). Behavior and counters are
// unchanged.
// =============================================================================

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
    /// Property streams whose names include the zero-based value index
    /// used by variable-length multiple-valued properties.
    indexed_property_streams_total: u64,
    /// Aggregate counts by MAPI property ID across every file scanned.
    /// Property IDs are a bounded, standard MAPI vocabulary, not user
    /// content. Excludes named-property-storage streams.
    property_id_counts: BTreeMap<u16, u64>,
    /// `__properties_version1.0` stream counts separated by the containing
    /// MS-OXMSG object scope. The named-property mapping storage is
    /// intentionally distinct from ordinary message/recipient/attachment
    /// property scopes.
    properties_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,
    /// `__substg1.0_*` property-stream counts separated by scope.
    property_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,
    /// Property ID counts separated by the containing MS-OXMSG object
    /// scope.
    property_id_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,

    attachment_storages_total: u64,
    recipient_storages_total: u64,
    named_property_storages_total: u64,
    embedded_object_storages_total: u64,

    /// Entries whose name matched none of the known MS-OXMSG conventions.
    /// Counted, never silently dropped: a nonzero count means either an
    /// MS-OXMSG structure this parser doesn't know about yet, or a real
    /// anomaly worth a closer look.
    unrecognized_entries_total: u64,
    /// Privacy-safe structural breakdown. Names and paths are never
    /// emitted.
    unrecognized_entries: BTreeMap<(CfbObjectKind, u64, UnrecognizedNameShape, String), u64>,
    /// A recognized MS-OXMSG name whose CFB object type is unexpected.
    recognized_name_type_mismatches: BTreeMap<RecognizedNameTypeMismatch, u64>,

    /// Entries beneath a custom (non-message-shaped) embedded-object
    /// storage. Their names are defined by the producing application, not
    /// MS-OXMSG ("Custom Attachment Storage"), so they are counted as
    /// opaque payload rather than matched against MS-OXMSG names.
    opaque_payload_entries_total: u64,
    /// (object kind, depth below the payload root) -> count.
    opaque_payload_entries: BTreeMap<(CfbObjectKind, u64), u64>,
    embedded_object_storages_message_shaped_total: u64,
    embedded_object_storages_custom_total: u64,
    /// (shape, storage CLSID) -> count. CLSIDs are a bounded
    /// class-identifier vocabulary, not user content.
    embedded_object_storages_by_shape: BTreeMap<(&'static str, String), u64>,

    // --- Property-type/value decoding --------------------------------------
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
    /// (scope, raw property type incl. the 0x1000 multi-value bit) ->
    /// count. Property types are a bounded MAPI vocabulary
    /// (MS-OXCDATA 2.11.1), not user content.
    property_type_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,
    /// Property Entry flags are a 3-bit MS-OXMSG vocabulary (mandatory /
    /// readable / writable), not user content.
    property_entry_flags_counts: BTreeMap<u32, u64>,
    /// Reserved-field values seen on the attachment-scope
    /// PidTagAttachDataObject (0x3701, PT_OBJECT) entry. Per MS-OXMSG
    /// 2.4.2.2, this is 0x01 for an embedded-message attachment and 0x04
    /// for a storage (OLE/custom) attachment -- an independent,
    /// property-level cross-check of the CFB-structural
    /// message-shaped/custom classification.
    attach_data_object_reserved_counts: BTreeMap<u32, u64>,
    /// Per spec this entry's Size field MUST be 0xFFFFFFFF; count any
    /// that aren't, rather than assuming.
    attach_data_object_size_sentinel_mismatches: u64,

    // --- Fixed-value and variable-value structural checks ------------------
    fixed_boolean_invalid_encoding_total: u64,
    /// A decoded PT_FLOAT/PT_DOUBLE/PT_APPTIME value that is NaN or
    /// infinite. Not necessarily invalid data on its own, but implausible
    /// for the values these types are normally used for -- worth
    /// investigating as a possible decode-path bug before assuming it's
    /// genuine.
    fixed_float_non_finite_total: u64,
    variable_value_stream_found_total: u64,
    variable_value_stream_missing_total: u64,
    variable_value_size_mismatch_total: u64,
    variable_value_odd_utf16_length_total: u64,
    /// PT_UNICODE bytes (already confirmed even-length) that still fail to
    /// decode as valid UTF-16 -- e.g. an unpaired surrogate.
    variable_unicode_decode_errors_total: u64,
    /// Bytes replaced with U+FFFD while decoding a PT_STRING8 value as
    /// Windows-1252 -- unverified against real data.
    variable_string8_undefined_byte_total: u64,
    /// PT_STRING8 value streams under a code page this decoder does not
    /// implement, with non-ASCII bytes (no text produced).
    variable_string8_unsupported_codepage_total: u64,
    /// PT_STRING8 value streams that are all ASCII under a known ASCII
    /// superset code page this decoder does not otherwise implement.
    variable_string8_ascii_under_unsupported_codepage_total: u64,
    /// Which link of the code page chain resolved, once per file.
    string8_codepage_from_message_total: u64,
    string8_codepage_from_internet_total: u64,
    string8_codepage_fallback_total: u64,
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
    /// Whether a string-kind named property's name decoded successfully --
    /// never the name itself.
    named_properties_string_decode_errors_total: u64,
    /// A resolved entry whose own claimed Property Index doesn't match the
    /// array position it was looked up by. Per MS-OXMSG this MUST always
    /// match; a permanent cross-check now that the bit layout is
    /// confirmed.
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

/// Everything `inspect_oxmsg` needs from a CFB entry, captured up front so
/// the immutable borrow from `comp.walk()` ends before the second pass
/// needs `&mut comp` to read stream contents.
struct CollectedOxmsgEntry {
    path: PathBuf,
    is_root: bool,
    is_stream: bool,
    len: u64,
    clsid: String,
}

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

    print_oxmsg_report(&totals);

    Ok(())
}

/// Prints the `--oxmsg` structural inventory report, separated from the
/// scan loop (SLAP). Keys are unchanged from previous versions.
fn print_oxmsg_report(totals: &OxmsgTotals) {
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
        "variable_string8_unsupported_codepage_total={}",
        totals.variable_string8_unsupported_codepage_total
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
        "named_properties_string_decode_errors_total={}",
        totals.named_properties_string_decode_errors_total
    );
    println!(
        "named_properties_index_mismatch_total={}",
        totals.named_properties_index_mismatch_total
    );
}

/// Orchestrates the two-pass structural diagnostic over one open .msg
/// container. Reports only structural fields -- type, ID, flags,
/// size/reserved, presence -- never a property's value.
fn inspect_oxmsg(comp: &mut CompoundFile, totals: &mut OxmsgTotals) {
    let message_shaped_parents = message_shaped_parent_paths(&*comp);

    // Pass 1 needs only immutable access; entries are collected up front,
    // ending that borrow before pass 2 needs `&mut comp` to read stream
    // contents.
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

    let saw_properties_stream = classify_oxmsg_entries(&entries, &message_shaped_parents, totals);
    if saw_properties_stream {
        totals.has_properties_stream += 1;
    }

    // Named properties are resolved once per file (the mapping storage is
    // shared by the whole message, embedded messages included) rather than
    // once per entry.
    let named_property_map = read_named_property_map(comp);

    // The PT_STRING8 code page is resolved once per file from the
    // top-level message's own properties.
    let string8_codepage = extract_string8_codepage(
        comp,
        Path::new("/"),
        properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32),
    );
    match string8_codepage.source {
        CodepageSource::MessageCodepage => totals.string8_codepage_from_message_total += 1,
        CodepageSource::InternetCodepage => totals.string8_codepage_from_internet_total += 1,
        CodepageSource::Fallback => totals.string8_codepage_fallback_total += 1,
    }

    decode_oxmsg_properties_streams(
        comp,
        &entries,
        named_property_map.as_ref(),
        string8_codepage.codepage,
        totals,
    );
}

/// Pass 1: classifies every CFB entry from its name, position, and
/// ancestry, updating the structural counters. Returns whether the file
/// had at least one `__properties_version1.0` stream.
fn classify_oxmsg_entries(
    entries: &[CollectedOxmsgEntry],
    message_shaped_parents: &BTreeSet<PathBuf>,
    totals: &mut OxmsgTotals,
) -> bool {
    let mut saw_properties_stream = false;

    for entry in entries {
        totals.total_entries += 1;
        let object_kind = cfb_object_kind(entry.is_stream);

        // Ancestry outranks name: anything beneath a custom attachment
        // storage is application-defined, whatever it happens to be called.
        if let Some(payload_root) =
            enclosing_custom_payload_root(&entry.path, message_shaped_parents)
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

    saw_properties_stream
}

/// Pass 2: decodes the entry array of every `__properties_version1.0`
/// stream found in pass 1. This reads stream contents, but reports only
/// structural fields -- never a property's value.
fn decode_oxmsg_properties_streams(
    comp: &mut CompoundFile,
    entries: &[CollectedOxmsgEntry],
    named_property_map: Option<&NamedPropertyMap>,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    for entry in entries {
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
        for prop_entry in &decoded.entries {
            record_property_entry(
                comp,
                &entry.path,
                scope,
                prop_entry,
                named_property_map,
                string8_codepage,
                totals,
            );
        }
    }
}

/// Runs every per-entry structural check for one decoded Property Entry:
/// type/flags accounting, fixed-value validity, the variable-length value
/// stream cross-check, the attach-data-object cross-check, and
/// named-property resolution.
fn record_property_entry(
    comp: &mut CompoundFile,
    stream_path: &Path,
    scope: OxmsgEntryScope,
    entry: &DecodedPropertyEntry,
    named_property_map: Option<&NamedPropertyMap>,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    totals.properties_entries_total += 1;
    *totals
        .property_type_counts_by_scope
        .entry((scope, entry.property_type))
        .or_insert(0) += 1;
    *totals
        .property_entry_flags_counts
        .entry(entry.flags)
        .or_insert(0) += 1;

    match classify_property_entry_shape(entry.property_type) {
        PropertyEntryShape::FixedInline => check_fixed_inline_entry(entry, totals),
        PropertyEntryShape::VariableSingle => {
            totals.properties_entries_variable_single_total += 1;
            check_variable_value_stream(comp, stream_path, entry, string8_codepage, totals);
        }
        PropertyEntryShape::VariableMultivalued => {
            totals.properties_entries_variable_multivalued_total += 1;
        }
    }

    record_attach_data_object_entry(scope, entry, totals);
    record_named_property_observation(entry.property_id, named_property_map, totals);
}

/// Structural checks for one fixed-length entry whose value is stored
/// inline in the entry's 8-byte value field. The value itself is decoded
/// only to check plausibility (non-finite floats) -- never read or
/// reported as content.
fn check_fixed_inline_entry(entry: &DecodedPropertyEntry, totals: &mut OxmsgTotals) {
    totals.properties_entries_fixed_inline_total += 1;
    if entry.property_type == 0x000B && !is_valid_boolean_encoding(&entry.tail) {
        totals.fixed_boolean_invalid_encoding_total += 1;
    }
    if let Some(value) = decode_fixed_value(entry.property_type, &entry.tail) {
        let non_finite = match value {
            DecodedFixedValue::Float(f) => !f.is_finite(),
            DecodedFixedValue::Double(d) | DecodedFixedValue::AppTime(d) => !d.is_finite(),
            _ => false,
        };
        if non_finite {
            totals.fixed_float_non_finite_total += 1;
        }
    }
}

/// Cross-checks one variable-length single-value entry against the
/// `__substg1.0_PPPPTTTT` stream MS-OXMSG 2.4.2.2 says must hold its
/// value: presence, declared-size agreement, and per-type decodability.
/// PT_OBJECT (0x000D) properties point at a storage, not a stream -- they
/// are already covered by the embedded-object accounting in pass 1, so
/// they are skipped here. Never reads a value as content.
fn check_variable_value_stream(
    comp: &mut CompoundFile,
    stream_path: &Path,
    entry: &DecodedPropertyEntry,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    let declared_size =
        u32::from_le_bytes([entry.tail[0], entry.tail[1], entry.tail[2], entry.tail[3]]);
    if entry.property_type == 0x000D || declared_size == 0xFFFF_FFFF {
        return;
    }
    let parent = stream_path.parent().unwrap_or(Path::new("/"));
    let value_path = expected_variable_stream_path(parent, entry.property_id, entry.property_type);
    let Some(value_bytes) = read_stream_bytes(comp, &value_path) else {
        totals.variable_value_stream_missing_total += 1;
        return;
    };

    totals.variable_value_stream_found_total += 1;
    let expected = expected_size_field_value(entry.property_type, value_bytes.len() as u64);
    if expected != declared_size as u64 {
        totals.variable_value_size_mismatch_total += 1;
    }
    match entry.property_type {
        0x001F => {
            if value_bytes.len() % 2 != 0 {
                totals.variable_value_odd_utf16_length_total += 1;
            } else if decode_unicode_value(&value_bytes).is_err() {
                totals.variable_unicode_decode_errors_total += 1;
            }
        }
        0x001E => match decode_string8_with_codepage(&value_bytes, string8_codepage) {
            String8Decoded::Decoded { replaced, .. } => {
                totals.variable_string8_undefined_byte_total += replaced as u64;
            }
            String8Decoded::AsciiUnderUnsupportedCodepage { .. } => {
                totals.variable_string8_ascii_under_unsupported_codepage_total += 1;
            }
            String8Decoded::UnsupportedCodepage => {
                totals.variable_string8_unsupported_codepage_total += 1;
            }
        },
        0x0048 if value_bytes.len() != 16 => {
            totals.variable_clsid_wrong_length_total += 1;
        }
        _ => {}
    }
}

/// PidTagAttachDataObject (0x3701, PT_OBJECT 0x000D) on an attachment: an
/// independent, property-level cross-check of the CFB-structural
/// message-shaped/custom classification (MS-OXMSG 2.4.2.2).
fn record_attach_data_object_entry(
    scope: OxmsgEntryScope,
    entry: &DecodedPropertyEntry,
    totals: &mut OxmsgTotals,
) {
    if scope != OxmsgEntryScope::Attachment
        || entry.property_id != 0x3701
        || entry.property_type != 0x000D
    {
        return;
    }
    let size = u32::from_le_bytes([entry.tail[0], entry.tail[1], entry.tail[2], entry.tail[3]]);
    let reserved = u32::from_le_bytes([entry.tail[4], entry.tail[5], entry.tail[6], entry.tail[7]]);
    *totals
        .attach_data_object_reserved_counts
        .entry(reserved)
        .or_insert(0) += 1;
    // Per spec this entry's Size field MUST be 0xFFFFFFFF; count any that
    // aren't, rather than assuming.
    if size != 0xFFFF_FFFF {
        totals.attach_data_object_size_sentinel_mismatches += 1;
    }
}

/// Named-property resolution (MS-OXMSG 2.2.3): identity only (property set
/// + numeric-or-string), never a string name.
fn record_named_property_observation(
    property_id: u16,
    named_property_map: Option<&NamedPropertyMap>,
    totals: &mut OxmsgTotals,
) {
    if property_id < 0x8000 {
        return;
    }
    totals.named_properties_seen_total += 1;
    let Some(map) = named_property_map else {
        totals.named_properties_map_missing_total += 1;
        return;
    };
    let Some(raw) = map.lookup(property_id) else {
        totals.named_properties_unresolvable_total += 1;
        return;
    };

    // Per MS-OXMSG the entry's own claimed Property Index MUST equal its
    // array position; a permanent cross-check now that the bit layout is
    // confirmed.
    let expected_index = property_id - 0x8000;
    if raw.property_index != expected_index {
        totals.named_properties_index_mismatch_total += 1;
    }
    if raw.is_string {
        totals.named_properties_string_kind_total += 1;
        if decode_named_property_string(&map.string_stream, raw.name_id_or_offset).is_none() {
            totals.named_properties_string_decode_errors_total += 1;
        }
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

// =============================================================================
// Custom MS-OXMSG extraction layer: real property reads used by --verify
// and --extract
//
// Unlike the --oxmsg structural diagnostic, these functions return real
// content. They are only ever consumed by --verify (which prints only
// match/mismatch counts) and --extract (which prints only the same
// counters the msg_parser diagnostic does) -- never printed directly.
// =============================================================================

/// Reads a string property by ID from `parent`: PT_UNICODE if present,
/// otherwise PT_STRING8 decoded under `string8_codepage`. `None` means
/// absent, undecodable, or under a code page this decoder does not
/// implement (never silently decoded as something else).
fn read_string_property(
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
fn extract_message_class(comp: &mut CompoundFile) -> Option<String> {
    let codepage = extract_string8_codepage(
        comp,
        Path::new("/"),
        properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32),
    );
    read_string_property(comp, Path::new("/"), PROP_MESSAGE_CLASS, codepage.codepage)
}

struct BodyFlags {
    has_plain: bool,
    has_html_native: bool,
    has_html_via_rtf: bool,
    has_rtf: bool,
    /// Mirrors [`RtfHtmlCheck::DecompressionFailed`] -- too short to be
    /// valid MS-OXRTFCP, or the crate itself returned an error.
    decompression_failed: bool,
    /// Byte length of the successfully-decompressed RTF, if decompression
    /// ran at all. `None` when there's no RTF or decompression failed --
    /// distinct from `Some(0)`, an empty-but-valid result.
    decompressed_rtf_len: Option<u64>,
}

/// Reads PidTagBody, PidTagBodyHtml, and PidTagRtfCompressed directly by
/// name, the custom-path equivalent of what `msg_parser`'s `Outlook`
/// exposes as `body`/`html`/`rtf_compressed`. Real content is read into
/// memory to run the HTML-in-RTF check, but nothing here is ever printed.
fn extract_body_flags(comp: &mut CompoundFile) -> BodyFlags {
    let root = Path::new("/");
    // Non-empty, not merely present: msg_parser's `body`/`html`/
    // `rtf_compressed` are empty for a zero-length (or absent) value, and
    // the `!is_empty()` checks this path is diffed against in
    // --verify/--extract use exactly that definition. A zero-length
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
struct RecipientTypeCounts {
    /// MS-OXOMSG value 0 -- the sender, recorded as a recipient.
    /// `msg_parser`'s `Outlook` has no field for this at all; there is
    /// nothing to compare it against, only to report.
    orig: u64,
    to: u64,
    cc: u64,
    bcc: u64,
    /// A `PidTagRecipientType` value outside the four defined ones.
    other: u64,
    /// A recipient storage whose type couldn't be read at all (missing
    /// properties stream, or no `0x0C15` entry in it).
    unresolved: u64,
}

/// Walks every top-level `__recip_version1.0_#*` storage and classifies
/// each by its own `PidTagRecipientType` (0x0C15, PT_LONG) fixed-length
/// entry. Real content stays in memory only as counts by category --
/// never a recipient's actual address or name, which this function never
/// reads at all.
fn extract_recipient_type_counts(comp: &mut CompoundFile) -> RecipientTypeCounts {
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
fn extract_attachment_count(comp: &CompoundFile) -> u64 {
    top_level_storage_paths(comp, OxmsgEntryKind::AttachmentStorage).len() as u64
}

#[derive(Default)]
struct AttachmentMethodCounts {
    by_value: u64,
    embedded_message: u64,
    ole: u64,
    other: u64,
    with_content_id: u64,
    /// An attachment storage whose PidTagAttachMethod couldn't be read at
    /// all (missing properties stream, or no 0x3705 entry in it).
    /// `msg_parser` has no equivalent bucket -- it always reports some
    /// method value -- so this is reported on its own, not folded into
    /// `other`.
    unresolved: u64,
    /// A by-value attachment whose PidTagAttachDataBinary is present but
    /// empty. A data stream that is absent or unreadable is NOT counted
    /// here -- see `zero_data_stream_missing` -- consistent with the
    /// no-silent-loss rule on `ZeroByteStats::record`.
    zero_byte_by_value: u64,
    /// A by-value attachment whose PidTagAttachDataBinary stream could
    /// not be read at all (absent, or a CFB read error). Reported on its
    /// own rather than folded into `zero_byte_by_value`: an unreadable
    /// stream is an anomaly, not evidence of an empty file.
    zero_data_stream_missing: u64,
    /// Paths of the attachment storages confirmed (from their own
    /// PidTagAttachMethod) to hold embedded messages, so callers can open
    /// each one rather than just knowing that at least one exists.
    embedded_paths: Vec<PathBuf>,
    /// A non-by-value attachment (or one whose method couldn't be read).
    /// Mirrors `msg_parser`'s own structural behavior: it leaves
    /// `payload_bytes` empty for every method other than by-value, since
    /// OLE and embedded-message content lives in a storage, not a flat
    /// stream.
    zero_size_other_method: u64,
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
fn extract_attachment_method_counts(comp: &mut CompoundFile) -> AttachmentMethodCounts {
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
struct OpenedEmbeddedMessage {
    class: Option<String>,
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
fn open_embedded_message(
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
fn run_msg_extract(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
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
fn inspect_oxmsg_as_msg(
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
    // embedded-message attachment it opens, and --extract promises to be
    // diffable against that report, so a message with two embedded
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
enum CountComparison {
    Match,
    Mismatch,
}

fn compare_count(msg_parser: u64, custom: u64) -> CountComparison {
    if msg_parser == custom {
        CountComparison::Match
    } else {
        CountComparison::Mismatch
    }
}

#[derive(Default)]
struct CountTally {
    matched: u64,
    mismatched: u64,
}

impl CountTally {
    fn record(&mut self, comparison: CountComparison) {
        match comparison {
            CountComparison::Match => self.matched += 1,
            CountComparison::Mismatch => self.mismatched += 1,
        }
    }
}

fn print_count_tally(name: &str, tally: &CountTally) {
    println!("{name}_match={}", tally.matched);
    println!("{name}_mismatch={}", tally.mismatched);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MessageClassComparison {
    BothPresentMatch,
    BothPresentMismatch,
    /// One path found a message class and the other didn't.
    PresenceMismatch,
    BothAbsent,
}

fn compare_message_class(msg_parser: Option<&str>, custom: Option<&str>) -> MessageClassComparison {
    match (msg_parser, custom) {
        (Some(a), Some(b)) if a == b => MessageClassComparison::BothPresentMatch,
        (Some(_), Some(_)) => MessageClassComparison::BothPresentMismatch,
        (None, None) => MessageClassComparison::BothAbsent,
        _ => MessageClassComparison::PresenceMismatch,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BoolFieldComparison {
    BothTrue,
    BothFalse,
    Mismatch,
}

fn compare_bool_field(msg_parser: bool, custom: bool) -> BoolFieldComparison {
    match (msg_parser, custom) {
        (true, true) => BoolFieldComparison::BothTrue,
        (false, false) => BoolFieldComparison::BothFalse,
        _ => BoolFieldComparison::Mismatch,
    }
}

#[derive(Default)]
struct BoolFieldTally {
    both_true: u64,
    both_false: u64,
    mismatch: u64,
}

impl BoolFieldTally {
    fn record(&mut self, comparison: BoolFieldComparison) {
        match comparison {
            BoolFieldComparison::BothTrue => self.both_true += 1,
            BoolFieldComparison::BothFalse => self.both_false += 1,
            BoolFieldComparison::Mismatch => self.mismatch += 1,
        }
    }
}

fn print_bool_field_tally(name: &str, tally: &BoolFieldTally) {
    println!("{name}_both_true={}", tally.both_true);
    println!("{name}_both_false={}", tally.both_false);
    println!("{name}_mismatch={}", tally.mismatch);
}

#[derive(Default)]
struct MsgVerifyTotals {
    open_errors_msg_parser: u64,
    open_errors_custom: u64,
    message_class_both_present_match: u64,
    message_class_both_present_mismatch: u64,
    message_class_presence_mismatch: u64,
    message_class_both_absent: u64,
    body_plain: BoolFieldTally,
    body_html_native: BoolFieldTally,
    body_html_via_rtf: BoolFieldTally,
    body_rtf: BoolFieldTally,
    msg_parser_rtf_decompression_errors: u64,
    custom_rtf_decompression_errors: u64,
    recipients_to: CountTally,
    recipients_cc: CountTally,
    recipients_bcc: CountTally,
    /// Custom-path-only: `msg_parser` has nothing to compare this against.
    /// This is the direct test of the "37 vs 36" recipient-count
    /// hypothesis.
    recipient_orig_total: u64,
    recipient_other_type_total: u64,
    recipient_unresolved_total: u64,
    attachments_total: CountTally,
    attachments_by_value: CountTally,
    attachments_embedded_message: CountTally,
    attachments_ole: CountTally,
    attachments_other: CountTally,
    attachments_with_content_id: CountTally,
    attachment_unresolved_total: u64,
    /// Custom-path-only: by-value attachments whose PidTagAttachDataBinary
    /// could not be read at all (absent or a CFB read error). Counted on
    /// its own -- an unreadable stream is an anomaly, not an empty file.
    attachment_data_stream_missing_total: u64,
    rtf_decompressed_bytes: CountTally,
    /// Byte deltas (custom minus msg_parser) for the rare case where
    /// decompressed RTF length disagrees -- not content, just a size
    /// difference, kept to confirm the magnitude matches what a narrow
    /// dictionary-region divergence would produce rather than something
    /// larger and less explicable.
    rtf_decompressed_byte_mismatch_deltas: Vec<i64>,
    embedded_message_class_readable_total: u64,
    embedded_message_class_unreadable_total: u64,
    /// The custom path's structural accounting over the same files,
    /// accumulated by `inspect_oxmsg` (formerly `--oxmsg`-only).
    structural: OxmsgTotals,
}

/// Runs both the custom extraction path and `msg_parser` over the same
/// files and compares their output field by field.
fn run_msg_verify(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
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
fn collect_msg_verify_totals(files: &[PathBuf]) -> MsgVerifyTotals {
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

        // Per embedded-message attachment, matching --extract; the
        // previous once-per-file gate undercounted any message with more
        // than one embedded-message attachment.
        let message_shaped = message_shaped_parent_paths(&comp);
        for attach_path in &attachment_methods.embedded_paths {
            match open_embedded_message(&mut comp, attach_path, &message_shaped) {
                Some(_) => totals.embedded_message_class_readable_total += 1,
                None => totals.embedded_message_class_unreadable_total += 1,
            }
        }

        // Structural accounting (formerly only reachable via --oxmsg):
        // every CFB entry classified, every properties stream and value
        // stream decoded. Content-free counters only.
        inspect_oxmsg(&mut comp, &mut totals.structural);
    }

    totals
}

/// Prints the `--verify` report, separated from the scan loop (SLAP).
fn print_msg_verify_report(totals: &MsgVerifyTotals) {
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
/// is visible. Names are the same output vocabulary `--oxmsg` used.
fn structural_gate_values(totals: &OxmsgTotals) -> Vec<(&'static str, i64)> {
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

fn count_structural_gate_violations(gates: &[(&'static str, i64)]) -> usize {
    gates.iter().filter(|(_, value)| *value != 0).count()
}

/// Prints the structural gates, a single violation count (0 on a clean
/// corpus), and a few informational counters that are reported but not
/// gated. Keys keep the names `--oxmsg` used.
fn print_verify_structural_gates(totals: &OxmsgTotals) {
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
        let path = std::env::temp_dir().join(format!(
            "tsp-synthetic-{}-{tag}.msg",
            std::process::id()
        ));
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
        let resolved =
            extract_string8_codepage(&mut comp, Path::new("/"), 32);
        assert_eq!(resolved.source, CodepageSource::Fallback);
        assert_eq!(extract_message_class(&mut comp).as_deref(), Some("IPM.Note"));
        drop(comp);
        let _ = std::fs::remove_file(&path);

        // Internet code page only.
        let path = write_synthetic_ansi_msg("internet", None, Some(65001), b"IPM.Note");
        let mut comp = cfb::open(&path).expect("open synthetic CFB");
        let resolved =
            extract_string8_codepage(&mut comp, Path::new("/"), 32);
        assert_eq!(
            resolved,
            ResolvedCodepage {
                codepage: 65001,
                source: CodepageSource::InternetCodepage
            }
        );
        assert_eq!(extract_message_class(&mut comp).as_deref(), Some("IPM.Note"));
        drop(comp);
        let _ = std::fs::remove_file(&path);

        // Message code page wins over the Internet code page, and an
        // all-ASCII class under a known ASCII superset still reads.
        let path = write_synthetic_ansi_msg("message", Some(932), Some(1251), b"IPM.Note");
        let mut comp = cfb::open(&path).expect("open synthetic CFB");
        let resolved =
            extract_string8_codepage(&mut comp, Path::new("/"), 32);
        assert_eq!(
            resolved,
            ResolvedCodepage {
                codepage: 932,
                source: CodepageSource::MessageCodepage
            }
        );
        assert_eq!(extract_message_class(&mut comp).as_deref(), Some("IPM.Note"));
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
}

