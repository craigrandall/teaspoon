//! Custom MS-OXMSG parser: property-stream and value decoding primitives, the `PT_STRING8` code
//! page chain, and named-property decoding.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::oxmsg_classify::OxmsgEntryScope;
use crate::shared::CompoundFile;

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
pub(crate) fn is_fixed_length_base_type(base_type: u16) -> bool {
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

pub(crate) const PROPERTY_TYPE_MULTIVALUE_BIT: u16 = 0x1000;

pub(crate) fn property_type_is_multivalued(property_type: u16) -> bool {
    property_type & PROPERTY_TYPE_MULTIVALUE_BIT != 0
}

#[derive(Clone, Copy)]
pub(crate) enum PropertyEntryShape {
    /// Value stored inline in the entry's 8-byte value field.
    FixedInline,
    /// Value stored in a separate `__substg1.0_PPPPTTTT` stream.
    VariableSingle,
    /// Values stored in a separate, indexed set of
    /// `__substg1.0_PPPPTTTT-NNNNNNNN` streams.
    VariableMultivalued,
}

pub(crate) fn classify_property_entry_shape(property_type: u16) -> PropertyEntryShape {
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
pub(crate) fn properties_stream_header_len(scope: OxmsgEntryScope) -> Option<usize> {
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
pub(crate) struct DecodedPropertyEntry {
    pub(crate) property_type: u16,
    pub(crate) property_id: u16,
    pub(crate) flags: u32,
    pub(crate) tail: [u8; 8],
}

pub(crate) struct DecodedPropertiesStream {
    pub(crate) entries: Vec<DecodedPropertyEntry>,
    /// Bytes remaining after the last full 16-byte entry. Always 0 for a
    /// well-formed stream.
    pub(crate) trailing_bytes: usize,
}

/// Parses the entry array of a `__properties_version1.0` stream. Returns
/// `None` only when `bytes` is shorter than `header_len`, which the caller
/// reports as an anomaly rather than silently skipping.
pub(crate) fn decode_properties_stream(
    bytes: &[u8],
    header_len: usize,
) -> Option<DecodedPropertiesStream> {
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
pub(crate) fn is_valid_boolean_encoding(tail: &[u8; 8]) -> bool {
    tail[1] == 0 && matches!(tail[0], 0 | 1)
}

/// A property value that fits inline in a Property Entry's 8-byte value
/// field, decoded to its real Rust type (MS-OXCDATA 2.11.1). Kept as a
/// typed, lossless intermediate representation -- not formatted for
/// display or written anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DecodedFixedValue {
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
pub(crate) fn decode_fixed_value(base_type: u16, tail: &[u8; 8]) -> Option<DecodedFixedValue> {
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
pub(crate) fn decode_unicode_value(bytes: &[u8]) -> Result<String, std::string::FromUtf16Error> {
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
pub(crate) fn cp1252_to_char(byte: u8) -> Option<char> {
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
pub(crate) fn decode_string8_cp1252(bytes: &[u8]) -> (String, u32) {
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
pub(crate) const PROP_MESSAGE_CLASS: u16 = 0x001A;

/// PidTagMessageCodepage (0x3FFD, PT_LONG): the code page used to encode
/// the non-Unicode (PT_STRING8) string properties of a message object
/// (Microsoft's own property page for it). Zero means "use the folder
/// object's code page", which a standalone `.msg` file cannot supply, so
/// zero is treated as unspecified rather than as a code page.
pub(crate) const PROP_MESSAGE_CODEPAGE: u16 = 0x3FFD;

/// PidTagInternetCodepage (0x3FDE, PT_LONG; PR_INTERNET_CPID): the
/// message's Internet code page. Second link in the chain: consulted only
/// when `PidTagMessageCodepage` is absent or zero. This ordering is a
/// documented teaspoon design choice, not a quotation of a specification
/// rule.
pub(crate) const PROP_INTERNET_CODEPAGE: u16 = 0x3FDE;

/// Last link in the chain: Windows-1252, the conventional default for
/// Western ANSI mail. Reported explicitly (never silently assumed) via
/// [`CodepageSource::Fallback`].
pub(crate) const STRING8_FALLBACK_CODEPAGE: u32 = 1252;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodepageSource {
    MessageCodepage,
    InternetCodepage,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedCodepage {
    pub(crate) codepage: u32,
    pub(crate) source: CodepageSource,
}

/// The PT_STRING8 code page resolution chain: `PidTagMessageCodepage`,
/// then `PidTagInternetCodepage`, then the documented Windows-1252
/// fallback. A value that is absent, zero, or negative is unspecified and
/// falls through to the next link.
pub(crate) fn resolve_string8_codepage(
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
pub(crate) fn read_long_property(
    decoded: &DecodedPropertiesStream,
    property_id: u16,
) -> Option<i32> {
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
pub(crate) fn extract_string8_codepage(
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
pub(crate) enum String8Decoded {
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
pub(crate) fn string8_codepage_is_ascii_superset(codepage: u32) -> bool {
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
pub(crate) fn decode_string8_with_codepage(bytes: &[u8], codepage: u32) -> String8Decoded {
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
pub(crate) fn decode_named_property_string(string_stream: &[u8], offset: u32) -> Option<String> {
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

pub(crate) fn expected_variable_stream_path(
    parent: &Path,
    property_id: u16,
    property_type: u16,
) -> PathBuf {
    parent.join(format!("__substg1.0_{property_id:04X}{property_type:04X}"))
}

/// MS-OXMSG 2.4.2.2: the declared Size field equals the value stream's
/// byte length for most types, +2 for PT_UNICODE, +1 for PT_STRING8.
pub(crate) fn expected_size_field_value(property_type: u16, actual_stream_len: u64) -> u64 {
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
pub(crate) const PSETID_ADDRESS: [u8; 16] = [
    0x04, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
pub(crate) const PSETID_APPOINTMENT: [u8; 16] = [
    0x02, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
pub(crate) const PSETID_COMMON: [u8; 16] = [
    0x08, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
pub(crate) const PSETID_LOG: [u8; 16] = [
    0x0A, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
pub(crate) const PSETID_NOTE: [u8; 16] = [
    0x0E, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
pub(crate) const PSETID_TASK: [u8; 16] = [
    0x03, 0x20, 0x06, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
/// {00020386-0000-0000-C000-000000000046} -- named properties synthesized
/// from MIME/internet-header fields. Confirmed present in real fixture
/// data during the bit-layout investigation.
pub(crate) const PS_INTERNET_HEADERS: [u8; 16] = [
    0x86, 0x03, 0x02, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];

pub(crate) fn classify_well_known_property_set(guid: &[u8; 16]) -> Option<&'static str> {
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
pub(crate) struct NamedPropertyEntryRaw {
    pub(crate) name_id_or_offset: u32,
    pub(crate) guid_index: u16,
    pub(crate) is_string: bool,
    /// The entry's own claimed array position. MS-OXMSG requires this to
    /// equal the entry's actual offset in the stream; kept so callers can
    /// cross-check it against the position they looked it up by.
    pub(crate) property_index: u16,
}

pub(crate) fn decode_named_property_entry(bytes: &[u8; 8]) -> NamedPropertyEntryRaw {
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

pub(crate) enum NamedPropertySet {
    PsMapi,
    PsPublicStrings,
    WellKnown(&'static str),
    Custom,
    /// `guid_index` pointed outside the GUID stream -- a real anomaly.
    OutOfRange,
}

pub(crate) struct NamedPropertyMap {
    pub(crate) guid_stream: Vec<u8>,
    pub(crate) entry_stream: Vec<u8>,
    pub(crate) string_stream: Vec<u8>,
}

impl NamedPropertyMap {
    pub(crate) fn lookup(&self, property_id: u16) -> Option<NamedPropertyEntryRaw> {
        let index = property_id.checked_sub(0x8000)? as usize;
        let offset = index * 8;
        let bytes = self.entry_stream.get(offset..offset + 8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(bytes);
        Some(decode_named_property_entry(&arr))
    }

    pub(crate) fn resolve_set(&self, guid_index: u16) -> NamedPropertySet {
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
pub(crate) fn read_stream_bytes(comp: &mut CompoundFile, path: &Path) -> Option<Vec<u8>> {
    let mut stream = comp.open_stream(path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Reads and parses the named property mapping storage, if present. Per
/// MS-OXMSG 2.2.3, this always lives at the top level, even for named
/// properties on an embedded message (Embedded Message objects MUST NOT
/// have their own).
pub(crate) fn read_named_property_map(comp: &mut CompoundFile) -> Option<NamedPropertyMap> {
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
