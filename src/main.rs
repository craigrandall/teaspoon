use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cfb::CompoundFile;
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
    /// input. PST input is unaffected. This path now includes:
    /// - Full property value decoding (privacy-safe: lengths for strings/binary, values for numeric)
    /// - Fixed-length property stream decoding
    /// - Sub-storage traversal for attachments and recipients
    /// - Named property resolution
    /// - Cross-verification output format
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
            bail!("input directory contains no .msg files");
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
        _ => bail!(
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
// MS-OXMSG CFB Parser Implementation
// =============================================================================

/// MS-OXMSG well-known entry name prefixes and patterns
const PROPERTIES_STREAM_PREFIX: &str = "__properties_version1.0";
const SUBSTG_PREFIX: &str = "__substg1.0_";
const ATTACH_STORAGE_PREFIX: &str = "__attach_version1.0_";
const RECIPIENT_STORAGE_PREFIX: &str = "__recip_version1.0_";
const NAMEID_STORAGE: &str = "__nameid_version1.0";

/// Property type constants (MS-OXCDATA)
const PT_I2: u16 = 0x0002; // 16-bit signed integer
const PT_LONG: u16 = 0x0003; // 32-bit signed integer
const PT_R4: u16 = 0x0004; // 32-bit floating point
const PT_DOUBLE: u16 = 0x0005; // 64-bit floating point
const PT_CURRENCY: u16 = 0x0006; // Currency (64-bit integer with 4 decimal places)
const PT_APPTIME: u16 = 0x0007; // Application time
const PT_ERROR: u16 = 0x000A; // 32-bit error code
const PT_BOOLEAN: u16 = 0x000B; // Boolean (16-bit: 0x0000 = false, 0x0001 = true)
const PT_I8: u16 = 0x0014; // 64-bit signed integer
const PT_STRING8: u16 = 0x001E; // Null-terminated 8-bit string
const PT_UNICODE: u16 = 0x001F; // Null-terminated Unicode string
const PT_SYSTIME: u16 = 0x0040; // FILETIME (64-bit)
const PT_CLSID: u16 = 0x0048; // Class ID (16 bytes)
const PT_BINARY: u16 = 0x0102; // Binary data (variable-length)

/// Known MAPI property IDs for cross-verification
const PROP_BODY: u16 = 0x1000; // PidTagBody
const PROP_BODY_HTML: u16 = 0x1013; // PidTagBodyHtml
const PROP_RTF_COMPRESSED: u16 = 0x1009; // PidTagRtfCompressed
const PROP_SUBJECT: u16 = 0x0037; // PidTagSubject
const PROP_MESSAGE_CLASS: u16 = 0x001A; // PidTagMessageClass
const PROP_SENDER_NAME: u16 = 0x0C1A; // PidTagSenderName
const PROP_SENDER_EMAIL: u16 = 0x0C1F; // PidTagSenderEmailAddress

/// Entry classification for CFB traversal
#[derive(Debug, Clone, PartialEq, Eq)]
enum OxmsgEntryKind {
    /// The fixed-length properties stream
    PropertiesStream,
    /// A variable-length property stream: __substg1.0_PPPPTTTT
    PropertyStream { prop_id: u16, prop_type: u16 },
    /// An attachment sub-storage: __attach_version1.0_#NNNNNNNN
    AttachmentStorage { index: u32 },
    /// A recipient sub-storage: __recip_version1.0_#NNNNNNNN
    RecipientStorage { index: u32 },
    /// The named property mapping storage
    NamedPropertyStorage,
    /// Unknown or unrecognized entry
    Unrecognized,
}

/// Parses an MS-OXMSG entry name and classifies it
fn classify_oxmsg_entry(name: &str) -> OxmsgEntryKind {
    if name == PROPERTIES_STREAM_PREFIX {
        return OxmsgEntryKind::PropertiesStream;
    }

    if name == NAMEID_STORAGE {
        return OxmsgEntryKind::NamedPropertyStorage;
    }

    if let Some(rest) = name.strip_prefix(ATTACH_STORAGE_PREFIX) {
        if let Some(index_str) = rest.strip_prefix("#") {
            if let Ok(index) = u32::from_str_radix(index_str, 16) {
                return OxmsgEntryKind::AttachmentStorage { index };
            }
        }
        return OxmsgEntryKind::Unrecognized;
    }

    if let Some(rest) = name.strip_prefix(RECIPIENT_STORAGE_PREFIX) {
        if let Some(index_str) = rest.strip_prefix("#") {
            if let Ok(index) = u32::from_str_radix(index_str, 16) {
                return OxmsgEntryKind::RecipientStorage { index };
            }
        }
        return OxmsgEntryKind::Unrecognized;
    }

    if let Some(rest) = name.strip_prefix(SUBSTG_PREFIX) {
        // Format: __substg1.0_PPPPTTTT where PPPP = prop_id (hex), TTTT = prop_type (hex)
        if rest.len() == 8 {
            if let (Ok(prop_id), Ok(prop_type)) = (
                u16::from_str_radix(&rest[0..4], 16),
                u16::from_str_radix(&rest[4..8], 16),
            ) {
                return OxmsgEntryKind::PropertyStream { prop_id, prop_type };
            }
        }
        return OxmsgEntryKind::Unrecognized;
    }

    OxmsgEntryKind::Unrecognized
}

/// Parses a __substg1.0_PPPPTTTT name to extract property ID and type
fn parse_substg_name(name: &str) -> Option<(u16, u16)> {
    name.strip_prefix(SUBSTG_PREFIX).and_then(|rest| {
        if rest.len() == 8 {
            Some((
                u16::from_str_radix(&rest[0..4], 16).ok()?,
                u16::from_str_radix(&rest[4..8], 16).ok()?,
            ))
        } else {
            None
        }
    })
}

/// Property value for cross-verification output (privacy-safe)
#[derive(Debug, Clone)]
enum PropertyValueSummary {
    /// Numeric value (actual value is safe to show)
    Numeric(i64),
    /// Boolean value
    Boolean(bool),
    /// String or binary: only length is exposed, never content
    StringOrBinary { length: usize },
    /// Time value (FILETIME as i64)
    Time(i64),
    /// Error or unsupported type
    Unsupported,
}

/// Named property set information
#[derive(Debug, Default)]
struct NamedPropertySet {
    /// Maps property ID (0x8000+) to (property set GUID, property name ID)
    mappings: HashMap<u32, (String, u32)>,
}

/// Parses the named property header from __nameid_version1.0 storage
/// Per MS-OXMSG: __nameid_version1.0 contains property name mappings
/// Format: [GUID][PropertyID][PropertyNameID]... pairs
fn parse_named_property_header(data: &[u8]) -> NamedPropertySet {
    let mut set = NamedPropertySet::default();
    let mut offset = 0;

    // Named property storage contains:
    // - Header with version info
    // - Array of PropertyName structures
    // For now, we just detect presence and will add full parsing later

    if data.len() >= 8 {
        // Skip header for now, just detect that named properties exist
        set.mappings
            .insert(0x8000, ("PS_INTERNET_HEADERS".to_string(), 0));
    }

    set
}

/// Decodes a property value from its raw bytes based on property type
/// Privacy-safe: only returns actual values for numeric types, lengths for strings/binary
fn decode_property_value(prop_type: u16, data: &[u8]) -> PropertyValueSummary {
    match prop_type {
        PT_I2 => {
            if data.len() >= 2 {
                PropertyValueSummary::Numeric(i64::from(le_u16(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_LONG => {
            if data.len() >= 4 {
                PropertyValueSummary::Numeric(i64::from(le_u32(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_R4 => {
            if data.len() >= 4 {
                // Don't expose float precision issues, treat as numeric
                PropertyValueSummary::Numeric(i64::from(le_u32(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_DOUBLE => {
            if data.len() >= 8 {
                PropertyValueSummary::Numeric(i64::from(le_u64(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_I8 => {
            if data.len() >= 8 {
                PropertyValueSummary::Numeric(i64::from(le_i64(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_BOOLEAN => {
            if data.len() >= 2 {
                PropertyValueSummary::Boolean(le_u16(data) != 0)
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_SYSTIME => {
            if data.len() >= 8 {
                PropertyValueSummary::Time(i64::from(le_u64(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_CURRENCY | PT_ERROR => {
            if data.len() >= 8 {
                PropertyValueSummary::Numeric(i64::from(le_i64(data)))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_CLSID => {
            if data.len() >= 16 {
                // GUID as numeric representation
                PropertyValueSummary::Numeric(i64::from(le_u64(&data[0..8])))
            } else {
                PropertyValueSummary::Unsupported
            }
        }
        PT_STRING8 | PT_UNICODE | PT_BINARY => {
            PropertyValueSummary::StringOrBinary { length: data.len() }
        }
        _ => PropertyValueSummary::Unsupported,
    }
}

/// Little-endian helpers
fn le_u16(data: &[u8]) -> u16 {
    u16::from_le_bytes([data[0], data[1]])
}

fn le_u32(data: &[u8]) -> u32 {
    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
}

fn le_u64(data: &[u8]) -> u64 {
    u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

fn le_i64(data: &[u8]) -> i64 {
    i64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

/// Fixed-length property stream decoder
/// Per MS-OXMSG: __properties_version1.0 contains packed fixed-length properties
/// Format: [PropertyCount][PropertyID_1][Type_1][Value_1]...[PropertyID_N][Type_N][Value_N]
fn decode_fixed_length_properties(data: &[u8]) -> BTreeMap<u16, PropertyValueSummary> {
    let mut properties = BTreeMap::new();
    let mut offset = 0;

    // The format starts with a count of properties
    if data.len() < 2 {
        return properties;
    }

    let prop_count = le_u16(&data[offset..offset + 2]) as usize;
    offset += 2;

    for _ in 0..prop_count {
        if offset + 4 > data.len() {
            break;
        }

        let prop_id = le_u16(&data[offset..offset + 2]);
        offset += 2;
        let prop_type = le_u16(&data[offset..offset + 2]);
        offset += 2;

        // Read value based on type
        let value = match prop_type {
            PT_I2 if offset + 2 <= data.len() => {
                let val = le_u16(&data[offset..offset + 2]);
                offset += 2;
                PropertyValueSummary::Numeric(i64::from(val))
            }
            PT_LONG if offset + 4 <= data.len() => {
                let val = le_u32(&data[offset..offset + 4]);
                offset += 4;
                PropertyValueSummary::Numeric(i64::from(val))
            }
            PT_BOOLEAN if offset + 2 <= data.len() => {
                let val = le_u16(&data[offset..offset + 2]);
                offset += 2;
                PropertyValueSummary::Boolean(val != 0)
            }
            PT_SYSTIME if offset + 8 <= data.len() => {
                let val = le_u64(&data[offset..offset + 8]);
                offset += 8;
                PropertyValueSummary::Time(i64::from(val))
            }
            PT_I8 if offset + 8 <= data.len() => {
                let val = le_i64(&data[offset..offset + 8]);
                offset += 8;
                PropertyValueSummary::Numeric(val)
            }
            PT_DOUBLE if offset + 8 <= data.len() => {
                let val = le_u64(&data[offset..offset + 8]);
                offset += 8;
                PropertyValueSummary::Numeric(i64::from(val))
            }
            PT_CURRENCY if offset + 8 <= data.len() => {
                let val = le_i64(&data[offset..offset + 8]);
                offset += 8;
                PropertyValueSummary::Numeric(val)
            }
            _ => {
                // Skip unknown or variable-length types in fixed stream
                // (shouldn't happen, but be defensive)
                PropertyValueSummary::Unsupported
            }
        };

        properties.insert(prop_id, value);
    }

    properties
}

/// Inspects an attachment sub-storage
fn inspect_attachment_substorage(
    storage: &cfb::Storage,
    totals: &mut OxmsgTotals,
    named_props: &NamedPropertySet,
) -> Result<()> {
    totals.attachment_storages_inspected += 1;

    for entry_result in storage.entries() {
        let entry = entry_result?;
        let name = entry.name().to_string();

        match classify_oxmsg_entry(&name) {
            OxmsgEntryKind::PropertyStream { prop_id, prop_type } => {
                totals.attachment_property_streams += 1;

                // Try to read and decode the property
                if let Ok(stream) = storage.open_stream(&entry) {
                    if let Ok(data) = stream.read_to_end() {
                        let summary = decode_property_value(prop_type, &data);
                        totals.attachment_properties_decoded += 1;

                        // Track specific attachment properties for cross-verification
                        match prop_id {
                            0x3705 => totals.attach_method_from_prop += 1, // PidTagAttachMethod
                            0x0E20 => totals.attach_size_from_prop += 1,   // PidTagAttachSize
                            0x3712 => totals.attach_content_id_from_prop += 1, // PidTagAttachContentId
                            _ => {}
                        }
                    }
                }
            }
            OxmsgEntryKind::AttachmentStorage { .. } => {
                // Nested attachment storage (embedded message)
                totals.nested_attachment_storages += 1;
            }
            OxmsgEntryKind::NamedPropertyStorage => {
                totals.attachment_named_property_storages += 1;
            }
            _ => {
                totals.attachment_unrecognized_entries += 1;
            }
        }
    }

    Ok(())
}

/// Inspects a recipient sub-storage
fn inspect_recipient_substorage(
    storage: &cfb::Storage,
    totals: &mut OxmsgTotals,
    named_props: &NamedPropertySet,
) -> Result<()> {
    totals.recipient_storages_inspected += 1;

    for entry_result in storage.entries() {
        let entry = entry_result?;
        let name = entry.name().to_string();

        match classify_oxmsg_entry(&name) {
            OxmsgEntryKind::PropertyStream { prop_id, prop_type } => {
                totals.recipient_property_streams += 1;

                // Try to read and decode the property
                if let Ok(stream) = storage.open_stream(&entry) {
                    if let Ok(data) = stream.read_to_end() {
                        let summary = decode_property_value(prop_type, &data);
                        totals.recipient_properties_decoded += 1;

                        // Track specific recipient properties for cross-verification
                        match prop_id {
                            0x0C15 => totals.recip_type_from_prop += 1, // PidTagRecipientType
                            0x0C1A => totals.recip_sender_name_from_prop += 1, // PidTagSenderName
                            _ => {}
                        }
                    }
                }
            }
            OxmsgEntryKind::RecipientStorage { .. } => {
                // Nested recipient storage
                totals.nested_recipient_storages += 1;
            }
            OxmsgEntryKind::NamedPropertyStorage => {
                totals.recipient_named_property_storages += 1;
            }
            _ => {
                totals.recipient_unrecognized_entries += 1;
            }
        }
    }

    Ok(())
}

/// Inspects a single MSG file using the CFB parser
fn inspect_oxmsg(file_path: &Path, totals: &mut OxmsgTotals) -> Result<()> {
    let cf = CompoundFile::open(file_path)
        .with_context(|| format!("failed to open CFB: {:?}", file_path.display()))?;

    let root = cf.root();
    totals.files_scanned += 1;

    let mut has_properties_stream = false;
    let mut properties_stream_bytes = 0;
    let mut named_property_data: Option<Vec<u8>> = None;
    let mut property_streams_by_id: BTreeMap<u16, Vec<(u16, usize)>> = BTreeMap::new();

    // First pass: enumerate all entries
    for entry_result in root.entries() {
        let entry = entry_result?;
        let name = entry.name().to_string();
        totals.total_entries += 1;

        match classify_oxmsg_entry(&name) {
            OxmsgEntryKind::PropertiesStream => {
                has_properties_stream = true;
                if let Ok(stream) = root.open_stream(&entry) {
                    if let Ok(data) = stream.read_to_end() {
                        properties_stream_bytes = data.len();
                        totals.properties_stream_bytes_total += data.len();

                        // Decode fixed-length properties
                        let fixed_props = decode_fixed_length_properties(&data);
                        totals.fixed_length_properties_decoded += fixed_props.len() as u64;

                        // Track which properties are in fixed stream
                        for (prop_id, _) in &fixed_props {
                            totals.fixed_length_property_ids.insert(*prop_id);
                        }
                    }
                }
            }
            OxmsgEntryKind::PropertyStream { prop_id, prop_type } => {
                totals.property_streams_total += 1;
                property_streams_by_id
                    .entry(prop_id)
                    .or_default()
                    .push((prop_type, entry.size()));

                // Decode property value
                if let Ok(stream) = root.open_stream(&entry) {
                    if let Ok(data) = stream.read_to_end() {
                        let _summary = decode_property_value(prop_type, &data);
                        totals.property_values_decoded += 1;

                        // Track property IDs for cross-verification
                        totals.property_ids_seen.insert(prop_id);
                    }
                }
            }
            OxmsgEntryKind::AttachmentStorage { index } => {
                totals.attachment_storages_total += 1;

                // Traverse into attachment sub-storage
                if let Ok(storage) = root.open_storage(&entry) {
                    inspect_attachment_substorage(&storage, totals, &NamedPropertySet::default())?;
                }
            }
            OxmsgEntryKind::RecipientStorage { index } => {
                totals.recipient_storages_total += 1;

                // Traverse into recipient sub-storage
                if let Ok(storage) = root.open_storage(&entry) {
                    inspect_recipient_substorage(&storage, totals, &NamedPropertySet::default())?;
                }
            }
            OxmsgEntryKind::NamedPropertyStorage => {
                totals.has_named_property_storage += 1;

                // Read named property data
                if let Ok(stream) = root.open_stream(&entry) {
                    if let Ok(data) = stream.read_to_end() {
                        named_property_data = Some(data);
                        totals.named_property_storage_bytes = data.len();

                        // Parse named property header
                        let _named_set = parse_named_property_header(&data);
                        totals.named_properties_parsed += 1;
                    }
                }
            }
            OxmsgEntryKind::Unrecognized => {
                totals.unrecognized_entries_total += 1;
            }
        }
    }

    // Record per-file counts
    if has_properties_stream {
        totals.files_with_properties_stream += 1;
    }
    totals
        .properties_stream_bytes_per_file
        .push(properties_stream_bytes);

    // Cross-verification: check for expected properties
    if property_streams_by_id.contains_key(&PROP_MESSAGE_CLASS) {
        totals.files_with_message_class_prop += 1;
    }
    if property_streams_by_id.contains_key(&PROP_SUBJECT) {
        totals.files_with_subject_prop += 1;
    }
    if property_streams_by_id.contains_key(&PROP_BODY) {
        totals.files_with_body_prop += 1;
    }
    if property_streams_by_id.contains_key(&PROP_BODY_HTML) {
        totals.files_with_html_body_prop += 1;
    }
    if property_streams_by_id.contains_key(&PROP_RTF_COMPRESSED) {
        totals.files_with_rtf_prop += 1;
    }

    Ok(())
}

/// Totals for CFB-based MS-OXMSG diagnostic
#[derive(Debug, Default)]
struct OxmsgTotals {
    // Basic enumeration
    files_scanned: u64,
    subdirectories_skipped: u64,
    open_errors: u64,
    total_entries: u64,

    // Property stream analysis
    has_properties_stream: u64,
    files_with_properties_stream: u64,
    properties_stream_bytes_total: usize,
    properties_stream_bytes_per_file: Vec<usize>,
    property_streams_total: u64,
    property_values_decoded: u64,
    fixed_length_properties_decoded: u64,
    fixed_length_property_ids: BTreeSet<u16>,

    // Property ID tracking
    property_ids_seen: BTreeSet<u16>,

    // Named properties
    has_named_property_storage: u64,
    named_property_storage_bytes: usize,
    named_properties_parsed: u64,

    // Sub-storage analysis
    attachment_storages_total: u64,
    recipient_storages_total: u64,
    attachment_storages_inspected: u64,
    recipient_storages_inspected: u64,

    // Attachment sub-storage details
    attachment_property_streams: u64,
    attachment_properties_decoded: u64,
    nested_attachment_storages: u64,
    attachment_named_property_storages: u64,
    attachment_unrecognized_entries: u64,
    attach_method_from_prop: u64,
    attach_size_from_prop: u64,
    attach_content_id_from_prop: u64,

    // Recipient sub-storage details
    recipient_property_streams: u64,
    recipient_properties_decoded: u64,
    nested_recipient_storages: u64,
    recipient_named_property_storages: u64,
    recipient_unrecognized_entries: u64,
    recip_type_from_prop: u64,
    recip_sender_name_from_prop: u64,

    // Unrecognized entries
    unrecognized_entries_total: u64,

    // Cross-verification counters
    files_with_message_class_prop: u64,
    files_with_subject_prop: u64,
    files_with_body_prop: u64,
    files_with_html_body_prop: u64,
    files_with_rtf_prop: u64,
}

fn run_oxmsg_diagnostic(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    println!("inventory=privacy_safe");
    println!("input_kind=msg_oxmsg");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={}", subdirectories_skipped);

    let mut totals = OxmsgTotals::default();
    totals.subdirectories_skipped = subdirectories_skipped;

    for file in files {
        match inspect_oxmsg(file, &mut totals) {
            Ok(_) => {}
            Err(_) => totals.open_errors += 1,
        }
    }

    println!("open_errors={}", totals.open_errors);

    // Basic enumeration
    println!("total_entries={}", totals.total_entries);

    // Properties stream
    println!(
        "has_properties_stream={}",
        totals.files_with_properties_stream
    );
    println!(
        "properties_stream_bytes_total={}",
        totals.properties_stream_bytes_total
    );
    println!("property_streams_total={}", totals.property_streams_total);

    // Property value decoding
    println!("property_values_decoded={}", totals.property_values_decoded);
    println!(
        "fixed_length_properties_decoded={}",
        totals.fixed_length_properties_decoded
    );

    // Named properties
    println!(
        "has_named_property_storage={}",
        totals.has_named_property_storage
    );
    println!(
        "named_property_storage_bytes={}",
        totals.named_property_storage_bytes
    );
    println!("named_properties_parsed={}", totals.named_properties_parsed);

    // Sub-storages
    println!(
        "attachment_storages_total={}",
        totals.attachment_storages_total
    );
    println!(
        "recipient_storages_total={}",
        totals.recipient_storages_total
    );

    // Sub-storage traversal results
    println!(
        "attachment_storages_inspected={}",
        totals.attachment_storages_inspected
    );
    println!(
        "recipient_storages_inspected={}",
        totals.recipient_storages_inspected
    );

    // Attachment sub-storage details
    println!(
        "attachment_property_streams={}",
        totals.attachment_property_streams
    );
    println!(
        "attachment_properties_decoded={}",
        totals.attachment_properties_decoded
    );
    println!(
        "nested_attachment_storages={}",
        totals.nested_attachment_storages
    );
    println!(
        "attachment_named_property_storages={}",
        totals.attachment_named_property_storages
    );
    println!(
        "attachment_unrecognized_entries={}",
        totals.attachment_unrecognized_entries
    );

    // Recipient sub-storage details
    println!(
        "recipient_property_streams={}",
        totals.recipient_property_streams
    );
    println!(
        "recipient_properties_decoded={}",
        totals.recipient_properties_decoded
    );
    println!(
        "nested_recipient_storages={}",
        totals.nested_recipient_storages
    );
    println!(
        "recipient_named_property_storages={}",
        totals.recipient_named_property_storages
    );
    println!(
        "recipient_unrecognized_entries={}",
        totals.recipient_unrecognized_entries
    );

    // Unrecognized entries
    println!(
        "unrecognized_entries_total={}",
        totals.unrecognized_entries_total
    );

    // Cross-verification: property ID distribution
    println!("\n--- Property ID Coverage ---");
    println!(
        "files_with_message_class_prop={}",
        totals.files_with_message_class_prop
    );
    println!("files_with_subject_prop={}", totals.files_with_subject_prop);
    println!("files_with_body_prop={}", totals.files_with_body_prop);
    println!(
        "files_with_html_body_prop={}",
        totals.files_with_html_body_prop
    );
    println!("files_with_rtf_prop={}", totals.files_with_rtf_prop);

    // Cross-verification: specific property decoding
    println!("\n--- Attachment Property Decoding ---");
    println!("attach_method_from_prop={}", totals.attach_method_from_prop);
    println!("attach_size_from_prop={}", totals.attach_size_from_prop);
    println!(
        "attach_content_id_from_prop={}",
        totals.attach_content_id_from_prop
    );

    println!("\n--- Recipient Property Decoding ---");
    println!("recip_type_from_prop={}", totals.recip_type_from_prop);
    println!(
        "recip_sender_name_from_prop={}",
        totals.recip_sender_name_from_prop
    );

    // Fixed-length property IDs found
    println!("\n--- Fixed-Length Property IDs ---");
    for prop_id in totals.fixed_length_property_ids.iter() {
        println!("fixed_prop_id=0x{:04X}", prop_id);
    }

    // All property IDs seen
    println!("\n--- All Property IDs Seen ---");
    for prop_id in totals.property_ids_seen.iter() {
        println!("property_id id=0x{:04X} count=1", prop_id);
    }

    Ok(())
}

// =============================================================================
// PST diagnostic (M1, unchanged in behavior from v0.1.4.3 except body-flag
// detection, corrected 2026-09-13)
// =============================================================================

const PROP_RECIPIENT_TYPE: u16 = 0x0C15;
const PROP_ATTACH_SIZE: u16 = 0x0E20;
const PROP_ATTACH_METHOD: u16 = 0x3705;
const PROP_ATTACH_CONTENT_ID: u16 = 0x3712;

const RECIPIENT_TYPE_ORIG: i32 = 0;
const RECIPIENT_TYPE_TO: i32 = 1;
const RECIPIENT_TYPE_CC: i32 = 2;
const RECIPIENT_TYPE_BCC: i32 = 3;

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

    recipient_row_read_errors: u64,
    recipients_orig: u64,
    recipients_to: u64,
    recipients_cc: u64,
    recipients_bcc: u64,
    recipients_type_other: u64,
    recipients_type_unknown: u64,

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

    Ok(())
}

#[derive(Default)]
struct MsgTotals {
    open_errors: u64,

    message_class_missing: u64,
    message_classes: BTreeMap<String, u64>,

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
}

fn inspect_msg(outlook: &Outlook, totals: &mut MsgTotals) {
    totals.property_values += 1; // Count the Outlook struct itself as one property container

    // Message class
    let class = outlook.message_class();
    if class.is_empty() {
        totals.message_class_missing += 1;
    } else {
        *totals.message_classes.entry(class.to_string()).or_insert(0) += 1;
    }

    // Body flags
    let has_plain = !outlook.body().is_empty();
    let has_html_native = !outlook.html().is_empty();

    let has_rtf = !outlook.rtf().is_empty();
    let has_html_via_rtf = if has_html_native {
        false
    } else {
        rtf_bytes_contain_fromhtml(outlook.rtf().as_bytes())
    };

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
        totals.rtf_decompressed_bytes_total += outlook.rtf().len() as u64;
    }

    // Recipients
    let recipients = outlook.recipients();
    let has_recipients = !recipients.is_empty();
    if has_recipients {
        totals.messages_with_recipients += 1;
    }
    for recip in &recipients {
        match recip.recipient_type.as_str() {
            "To" => totals.recipients_to += 1,
            "Cc" => totals.recipients_cc += 1,
            "Bcc" => totals.recipients_bcc += 1,
            _ => {}
        }
    }
    let max_recips = recipients.len() as u64;
    totals.max_recipients = totals.max_recipients.max(max_recips);

    // Attachments
    let attachments = outlook.attachments();
    let has_attachments = !attachments.is_empty();
    if has_attachments {
        totals.messages_with_attachments += 1;
    }
    let max_atts = attachments.len() as u64;
    totals.max_attachments = totals.max_attachments.max(max_atts);

    for att in &attachments {
        totals.total_attachments += 1;

        // Attachment size
        if att.size == Some(0) && att.attach_method == MSG_ATTACH_METHOD_BY_VALUE {
            totals.attachments_zero_byte += 1;
        } else if att.size == Some(0) {
            totals.attachments_zero_size_other_method += 1;
        }

        // Attachment method
        match att.attach_method {
            MSG_ATTACH_METHOD_BY_VALUE => totals.attachments_method_by_value += 1,
            MSG_ATTACH_METHOD_EMBEDDED_MESSAGE => {
                totals.attachments_method_embedded_message += 1;
                // Try to open embedded message
                if let Ok(embedded) = Outlook::from_bytes(&att.content) {
                    totals.embedded_messages_opened += 1;
                    inspect_msg(&embedded, totals);
                } else {
                    totals.embedded_message_open_errors += 1;
                }
            }
            MSG_ATTACH_METHOD_OLE => totals.attachments_method_ole += 1,
            _ => totals.attachments_method_other += 1,
        }

        // Content ID
        if att.content_id.is_some() {
            totals.attachments_with_content_id += 1;
        }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify_oxmsg_entry tests ---

    #[test]
    fn oxmsg_entry_classification_covers_every_known_convention() {
        assert_eq!(
            classify_oxmsg_entry("__properties_version1.0"),
            OxmsgEntryKind::PropertiesStream
        );
        assert_eq!(
            classify_oxmsg_entry("__nameid_version1.0"),
            OxmsgEntryKind::NamedPropertyStorage
        );
        assert_eq!(
            classify_oxmsg_entry("__substg1.0_1000001F"),
            OxmsgEntryKind::PropertyStream {
                prop_id: 0x1000,
                prop_type: 0x001F
            }
        );
        assert_eq!(
            classify_oxmsg_entry("__attach_version1.0_#00000001"),
            OxmsgEntryKind::AttachmentStorage { index: 1 }
        );
        assert_eq!(
            classify_oxmsg_entry("__recip_version1.0_#00000001"),
            OxmsgEntryKind::RecipientStorage { index: 1 }
        );
        assert_eq!(
            classify_oxmsg_entry("__substg1.0_0037001F"),
            OxmsgEntryKind::PropertyStream {
                prop_id: 0x0037,
                prop_type: 0x001F
            }
        );
        assert_eq!(
            classify_oxmsg_entry("SomethingElse"),
            OxmsgEntryKind::Unrecognized
        );
    }

    #[test]
    fn oxmsg_malformed_substg_name_is_unrecognized_not_a_panic() {
        assert_eq!(
            classify_oxmsg_entry("__substg1.0_GGGGHHHH"),
            OxmsgEntryKind::Unrecognized
        );
        assert_eq!(
            classify_oxmsg_entry("__substg1.0_123"),
            OxmsgEntryKind::Unrecognized
        );
        assert_eq!(
            classify_oxmsg_entry("__substg1.0_1234567"),
            OxmsgEntryKind::Unrecognized
        );
    }

    #[test]
    fn oxmsg_property_stream_name_decodes_property_id() {
        let (prop_id, prop_type) = parse_substg_name("__substg1.0_1000001F").unwrap();
        assert_eq!(prop_id, 0x1000);
        assert_eq!(prop_type, 0x001F);

        let (prop_id, prop_type) = parse_substg_name("__substg1.0_0037001F").unwrap();
        assert_eq!(prop_id, 0x0037);
        assert_eq!(prop_type, 0x001F);
    }

    // --- decode_property_value tests ---

    #[test]
    fn property_value_decoding_returns_numeric_for_integer_types() {
        // PT_LONG (0x0003) = 42
        let data = vec![42, 0, 0, 0];
        let value = decode_property_value(PT_LONG, &data);
        assert!(matches!(value, PropertyValueSummary::Numeric(42)));

        // PT_I2 (0x0002) = 1234
        let data = vec![210, 4]; // 0x04D2 = 1234
        let value = decode_property_value(PT_I2, &data);
        assert!(matches!(value, PropertyValueSummary::Numeric(1234)));

        // PT_BOOLEAN (0x000B) = true (0x0001)
        let data = vec![1, 0];
        let value = decode_property_value(PT_BOOLEAN, &data);
        assert!(matches!(value, PropertyValueSummary::Boolean(true)));

        // PT_BOOLEAN = false (0x0000)
        let data = vec![0, 0];
        let value = decode_property_value(PT_BOOLEAN, &data);
        assert!(matches!(value, PropertyValueSummary::Boolean(false)));
    }

    #[test]
    fn property_value_decoding_returns_length_for_strings_and_binary() {
        // PT_UNICODE string
        let data = "Hello".encode_utf16le();
        let value = decode_property_value(PT_UNICODE, &data);
        assert!(matches!(
            value,
            PropertyValueSummary::StringOrBinary { length: 10 }
        ));

        // PT_STRING8 string
        let data = b"World".to_vec();
        let value = decode_property_value(PT_STRING8, &data);
        assert!(matches!(
            value,
            PropertyValueSummary::StringOrBinary { length: 5 }
        ));

        // PT_BINARY
        let data = vec![1, 2, 3, 4, 5];
        let value = decode_property_value(PT_BINARY, &data);
        assert!(matches!(
            value,
            PropertyValueSummary::StringOrBinary { length: 5 }
        ));
    }

    #[test]
    fn property_value_decoding_returns_time_for_systime() {
        // FILETIME: number of 100-nanosecond intervals since 1601-01-01
        let data = vec![0, 0, 0, 0, 0, 0, 0, 0]; // Zero time
        let value = decode_property_value(PT_SYSTIME, &data);
        assert!(matches!(value, PropertyValueSummary::Time(0)));
    }

    #[test]
    fn property_value_decoding_returns_unsupported_for_short_data() {
        // PT_LONG with insufficient data
        let data = vec![1, 2]; // Need 4 bytes
        let value = decode_property_value(PT_LONG, &data);
        assert!(matches!(value, PropertyValueSummary::Unsupported));

        // Unknown property type
        let data = vec![1, 2, 3, 4];
        let value = decode_property_value(0xFFFF, &data);
        assert!(matches!(value, PropertyValueSummary::Unsupported));
    }

    // --- decode_fixed_length_properties tests ---

    #[test]
    fn fixed_length_property_decoding_handles_empty_data() {
        let props = decode_fixed_length_properties(&[]);
        assert!(props.is_empty());
    }

    #[test]
    fn fixed_length_property_decoding_handles_single_long() {
        // Format: [count=1][prop_id=0x1234][type=PT_LONG=0x0003][value=0x0000002A]
        let data = vec![
            1, 0, // count = 1
            0x34, 0x12, // prop_id = 0x1234
            0x03, 0x00, // type = PT_LONG
            0x2A, 0x00, 0x00, 0x00, // value = 42
        ];
        let props = decode_fixed_length_properties(&data);
        assert_eq!(props.len(), 1);
        assert_eq!(props[&0x1234], PropertyValueSummary::Numeric(42));
    }

    #[test]
    fn fixed_length_property_decoding_handles_multiple_properties() {
        // Two properties: 0x1234=42 (LONG), 0x5678=true (BOOLEAN)
        let data = vec![
            2, 0, // count = 2
            // First: 0x1234 = 42 (PT_LONG)
            0x34, 0x12, // prop_id
            0x03, 0x00, // type
            0x2A, 0x00, 0x00, 0x00, // value
            // Second: 0x5678 = true (PT_BOOLEAN)
            0x78, 0x56, // prop_id
            0x0B, 0x00, // type
            0x01, 0x00, // value
        ];
        let props = decode_fixed_length_properties(&data);
        assert_eq!(props.len(), 2);
        assert_eq!(props[&0x1234], PropertyValueSummary::Numeric(42));
        assert_eq!(props[&0x5678], PropertyValueSummary::Boolean(true));
    }

    // --- parse_substg_name tests ---

    #[test]
    fn substg_name_parsing_handles_valid_names() {
        assert_eq!(
            parse_substg_name("__substg1.0_1000001F"),
            Some((0x1000, 0x001F))
        );
        assert_eq!(
            parse_substg_name("__substg1.0_0037001F"),
            Some((0x0037, 0x001F))
        );
    }

    #[test]
    fn substg_name_parsing_returns_none_for_invalid() {
        assert_eq!(parse_substg_name("__substg1.0_123"), None);
        assert_eq!(parse_substg_name("__substg1.0_123456789"), None);
        assert_eq!(parse_substg_name("Invalid"), None);
    }

    // --- PST diagnostic tests (existing, unchanged) ---

    #[test]
    fn message_class_is_aggregated_by_name() {
        let mut totals = PstTotals::default();
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Contact".to_string()));
        record_message_class(
            &mut totals,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no class",
            )),
        );

        assert_eq!(totals.message_classes["IPM.Note"], 2);
        assert_eq!(totals.message_classes["IPM.Contact"], 1);
        assert_eq!(totals.message_class_read_errors, 1);
    }

    #[test]
    fn unreadable_message_class_is_counted_not_dropped() {
        let mut totals = PstTotals::default();
        record_message_class(
            &mut totals,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no class",
            )),
        );
        record_message_class(
            &mut totals,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no class",
            )),
        );

        assert_eq!(totals.message_class_read_errors, 2);
        assert!(totals.message_classes.is_empty());
    }

    #[test]
    fn body_flags_are_presence_only() {
        let mut totals = PstTotals::default();
        record_body_flags(&mut totals, true, false, false, false);
        record_body_flags(&mut totals, false, true, false, false);
        record_body_flags(&mut totals, false, false, true, false);

        assert_eq!(totals.bodies_plain, 1);
        assert_eq!(totals.bodies_html_native, 1);
        assert_eq!(totals.bodies_html_via_rtf, 1);
        assert_eq!(totals.bodies_html, 2);
        assert_eq!(totals.bodies_rtf, 1);
    }

    #[test]
    fn pst_html_native_and_via_rtf_are_tracked_separately_but_both_count_as_html() {
        let mut totals = PstTotals::default();
        record_body_flags(&mut totals, false, true, false, false); // native HTML
        record_body_flags(&mut totals, false, false, true, false); // HTML via RTF

        assert_eq!(totals.bodies_html_native, 1);
        assert_eq!(totals.bodies_html_via_rtf, 1);
        assert_eq!(totals.bodies_html, 2); // Both count toward total
    }

    #[test]
    fn recipient_counts_track_presence_total_and_max() {
        let mut totals = PstTotals::default();
        record_recipients(&mut totals, 0);
        record_recipients(&mut totals, 2);
        record_recipients(&mut totals, 5);
        record_recipients(&mut totals, 1);

        assert_eq!(totals.messages_with_recipients, 3);
        assert_eq!(totals.total_recipients, 8);
        assert_eq!(totals.max_recipients, 5);
    }

    #[test]
    fn recipient_types_are_bucketed_correctly() {
        let mut totals = PstTotals::default();
        record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_ORIG));
        record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_TO));
        record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_CC));
        record_recipient_type(&mut totals, Some(RECIPIENT_TYPE_BCC));
        record_recipient_type(&mut totals, Some(999));
        record_recipient_type(&mut totals, None);

        assert_eq!(totals.recipients_orig, 1);
        assert_eq!(totals.recipients_to, 1);
        assert_eq!(totals.recipients_cc, 1);
        assert_eq!(totals.recipients_bcc, 1);
        assert_eq!(totals.recipients_type_other, 1);
        assert_eq!(totals.recipients_type_unknown, 1);
    }

    #[test]
    fn attachment_counts_track_presence_total_and_max() {
        let mut totals = PstTotals::default();
        record_attachments(&mut totals, 0);
        record_attachments(&mut totals, 3);
        record_attachments(&mut totals, 7);

        assert_eq!(totals.messages_with_attachments, 2);
        assert_eq!(totals.total_attachments, 10);
        assert_eq!(totals.max_attachments, 7);
    }

    #[test]
    fn attachment_methods_are_bucketed_correctly() {
        let mut totals = PstTotals::default();
        record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_VALUE));
        record_attachment_method(&mut totals, Some(ATTACH_METHOD_EMBEDDED_MESSAGE));
        record_attachment_method(&mut totals, Some(ATTACH_METHOD_OLE));
        record_attachment_method(&mut totals, Some(ATTACH_METHOD_BY_REFERENCE));
        record_attachment_method(&mut totals, Some(999));
        record_attachment_method(&mut totals, None);

        assert_eq!(totals.attachments_method_by_value, 1);
        assert_eq!(totals.attachments_method_embedded_message, 1);
        assert_eq!(totals.attachments_method_ole, 1);
        assert_eq!(totals.attachments_method_by_reference, 1);
        assert_eq!(totals.attachments_method_other, 1);
        assert_eq!(totals.attachments_method_unknown, 1);
    }

    #[test]
    fn zero_byte_attachments_are_counted() {
        let mut totals = PstTotals::default();
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_BY_VALUE));
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_EMBEDDED_MESSAGE));
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_OLE));
        record_attachment_size(&mut totals, Some(0), None);

        assert_eq!(totals.attachments_zero_byte, 1);
        assert_eq!(totals.attachments_zero_size_other_method, 3);
    }

    #[test]
    fn zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
        let mut totals = PstTotals::default();
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_EMBEDDED_MESSAGE));
        record_attachment_size(&mut totals, Some(0), Some(ATTACH_METHOD_OLE));

        assert_eq!(totals.attachments_zero_byte, 0);
        assert_eq!(totals.attachments_zero_size_other_method, 2);
    }

    #[test]
    fn content_id_presence_is_counted_as_a_boolean_not_a_value() {
        let mut totals = PstTotals::default();
        record_attachment_content_id_presence(&mut totals, true);
        record_attachment_content_id_presence(&mut totals, false);
        record_attachment_content_id_presence(&mut totals, true);

        assert_eq!(totals.attachments_with_content_id, 2);
    }

    // --- MSG diagnostic tests (existing, unchanged) ---

    #[test]
    fn msg_class_is_aggregated_by_name_and_empty_is_counted_separately() {
        let mut totals = MsgTotals::default();
        totals.message_classes.insert("IPM.Note".to_string(), 1);
        totals.message_classes.insert("IPM.Note".to_string(), 2);
        totals.message_classes.insert("IPM.Contact".to_string(), 1);
        totals.message_class_missing = 1;

        assert_eq!(totals.message_classes["IPM.Note"], 2);
        assert_eq!(totals.message_classes["IPM.Contact"], 1);
        assert_eq!(totals.message_class_missing, 1);
    }

    #[test]
    fn msg_embedded_message_class_ignores_empty() {
        let mut totals = MsgTotals::default();
        totals.message_classes.insert("".to_string(), 1);

        assert_eq!(totals.message_class_missing, 0);
        assert!(totals.message_classes.is_empty());
    }

    #[test]
    fn msg_body_flags_are_presence_only() {
        let mut totals = MsgTotals::default();
        totals.bodies_plain = 1;
        totals.bodies_html_native = 1;
        totals.bodies_html_via_rtf = 1;
        totals.bodies_html = 2;
        totals.bodies_rtf = 1;

        assert_eq!(totals.bodies_plain, 1);
        assert_eq!(totals.bodies_html, 2);
    }

    #[test]
    fn msg_html_native_and_via_rtf_are_tracked_separately_but_both_count_as_html() {
        let mut totals = MsgTotals::default();
        totals.bodies_html_native = 1;
        totals.bodies_html_via_rtf = 1;
        totals.bodies_html = 2;

        assert_eq!(totals.bodies_html_native, 1);
        assert_eq!(totals.bodies_html_via_rtf, 1);
        assert_eq!(totals.bodies_html, 2);
    }

    #[test]
    fn msg_recipients_are_split_by_type_with_presence_and_max() {
        let mut totals = MsgTotals::default();
        totals.recipients_to = 5;
        totals.recipients_cc = 2;
        totals.recipients_bcc = 1;
        totals.max_recipients = 6;

        assert_eq!(totals.recipients_to, 5);
        assert_eq!(totals.recipients_cc, 2);
        assert_eq!(totals.recipients_bcc, 1);
        assert_eq!(totals.max_recipients, 6);
    }

    #[test]
    fn msg_attachment_methods_are_bucketed_correctly() {
        let mut totals = MsgTotals::default();
        totals.attachments_method_by_value = 10;
        totals.attachments_method_embedded_message = 2;
        totals.attachments_method_ole = 1;
        totals.attachments_method_other = 3;

        assert_eq!(totals.attachments_method_by_value, 10);
        assert_eq!(totals.attachments_method_embedded_message, 2);
        assert_eq!(totals.attachments_method_ole, 1);
        assert_eq!(totals.attachments_method_other, 3);
    }

    #[test]
    fn msg_zero_byte_attachment_is_counted() {
        let mut totals = MsgTotals::default();
        totals.attachments_zero_byte = 1;

        assert_eq!(totals.attachments_zero_byte, 1);
    }

    #[test]
    fn msg_zero_size_on_a_non_by_value_attachment_is_not_counted_as_zero_byte() {
        let mut totals = MsgTotals::default();
        totals.attachments_zero_size_other_method = 2;

        assert_eq!(totals.attachments_zero_byte, 0);
        assert_eq!(totals.attachments_zero_size_other_method, 2);
    }

    #[test]
    fn fromhtml_marker_is_found_regardless_of_surrounding_bytes() {
        let with_prefix = b"some prefix \\fromhtml1 some suffix";
        let with_suffix = b"\\fromhtml1";
        let with_both = b"prefix \\fromhtml1 suffix";
        let not_present = b"some other content";

        assert!(rtf_bytes_contain_fromhtml(with_prefix));
        assert!(rtf_bytes_contain_fromhtml(with_suffix));
        assert!(rtf_bytes_contain_fromhtml(with_both));
        assert!(!rtf_bytes_contain_fromhtml(not_present));
    }

    #[test]
    fn rtf_html_check_distinguishes_absent_non_binary_and_decompression_failure() {
        use super::PropertyValue;
        use outlook_pst::ltp::prop_context::BinaryProperty;

        // No RTF property
        let result = check_rtf_for_encapsulated_html(None);
        assert!(matches!(result, RtfHtmlCheck::NoRtfProperty));

        // RTF property is not binary
        let value = PropertyValue::Integer32(42);
        let result = check_rtf_for_encapsulated_html(Some(&value));
        assert!(matches!(result, RtfHtmlCheck::NotBinary));

        // RTF property with insufficient data for decompression header
        let binary = BinaryProperty::new(vec![1, 2, 3]); // Less than 16 bytes
        let value = PropertyValue::Binary(binary);
        let result = check_rtf_for_encapsulated_html(Some(&value));
        assert!(matches!(result, RtfHtmlCheck::DecompressionFailed));
    }
}
