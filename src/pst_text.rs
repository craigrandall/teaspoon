//! Text from PST string properties (M4i, decision D4).
//!
//! `outlook-pst`'s `String8Value::to_string()` turns every byte into the character with the same
//! number, which is Latin-1: not Windows-1252 and not the store's code page. ASCII is exact, but
//! bytes 0x80 to 0x9F become control characters and a multibyte code page becomes garbage, with
//! nothing counted. So no PST string is read through `to_string()` on a `String8` value here.
//! A string-typed value is read through [`TextReader::text`]: a UTF-16 value as it is, and a
//! `PT_STRING8` value from its raw bytes through the chain the `.msg` path already uses and
//! verified (the message code page, else the Internet code page, else Windows-1252;
//! `oxmsg_decode`), with every loss counted: bytes with no mapping, and text under a code page
//! the decoder does not implement (no text is produced for it).
//!
//! The reader holds the code page of the message being read, so a caller sets it once per
//! message (`use_message_codepage`) and reads that message's strings; folder names, which have
//! no message, use the Windows-1252 fallback.

use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::ltp::table_context::{TableContext, TableContextInfo, TableRowColumnValue};

use crate::oxmsg_decode::{
    STRING8_FALLBACK_CODEPAGE, String8Decoded, decode_string8_with_codepage,
    resolve_string8_codepage,
};

/// Content-free counts of the `PT_STRING8` values read and of what was lost reading them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextStats {
    /// `PT_STRING8` values read.
    pub(crate) string8_values: u64,
    /// Of those, values with at least one byte above 0x7F.
    pub(crate) string8_non_ascii: u64,
    /// Bytes with no mapping in the code page used (each became U+FFFD).
    pub(crate) string8_replaced_bytes: u64,
    /// Values under a code page the decoder does not implement, with non-ASCII bytes: no text
    /// was produced.
    pub(crate) string8_unsupported: u64,
}

impl TextStats {
    pub(crate) fn add(&mut self, other: &TextStats) {
        self.string8_values += other.string8_values;
        self.string8_non_ascii += other.string8_non_ascii;
        self.string8_replaced_bytes += other.string8_replaced_bytes;
        self.string8_unsupported += other.string8_unsupported;
    }
}

/// Reads string-typed property values under one code page and counts the loss.
#[derive(Debug, Clone)]
pub(crate) struct TextReader {
    /// The code page a `PT_STRING8` value is decoded under.
    pub(crate) codepage: u32,
    pub(crate) stats: TextStats,
}

impl TextReader {
    /// A reader under the Windows-1252 fallback (the right start for a folder name).
    pub(crate) fn new() -> Self {
        TextReader {
            codepage: STRING8_FALLBACK_CODEPAGE,
            stats: TextStats::default(),
        }
    }

    /// Back to the Windows-1252 fallback, for text that belongs to no message (a folder name).
    pub(crate) fn use_fallback_codepage(&mut self) {
        self.codepage = STRING8_FALLBACK_CODEPAGE;
    }

    /// Sets the code page from a message's own properties: `PidTagMessageCodepage`, else
    /// `PidTagInternetCodepage`, else Windows-1252 (zero, negative, and absent values fall
    /// through).
    pub(crate) fn use_message_codepage(
        &mut self,
        message_codepage: Option<i32>,
        internet_codepage: Option<i32>,
    ) {
        self.codepage = resolve_string8_codepage(message_codepage, internet_codepage).codepage;
    }

    /// The text of a string-typed value, or `None` for any other type and for `PT_STRING8`
    /// text under a code page the decoder does not implement (counted).
    pub(crate) fn text(&mut self, value: &PropertyValue) -> Option<String> {
        match value {
            PropertyValue::Unicode(v) => Some(v.to_string()),
            PropertyValue::String8(v) => {
                let bytes = v.buffer();
                self.stats.string8_values += 1;
                if !bytes.is_ascii() {
                    self.stats.string8_non_ascii += 1;
                }
                match decode_string8_with_codepage(bytes, self.codepage) {
                    String8Decoded::Decoded { text, replaced } => {
                        self.stats.string8_replaced_bytes += u64::from(replaced);
                        Some(text)
                    }
                    String8Decoded::AsciiUnderUnsupportedCodepage { text } => Some(text),
                    String8Decoded::UnsupportedCodepage => {
                        self.stats.string8_unsupported += 1;
                        None
                    }
                }
            }
            _ => None,
        }
    }
}

/// The 32-bit integer of a property value, or `None` for any other type or an absent value.
pub(crate) fn integer32(value: Option<&PropertyValue>) -> Option<i32> {
    match value {
        Some(PropertyValue::Integer32(v)) => Some(*v),
        _ => None,
    }
}

/// The text in one column of one table row, read through `text`.
pub(crate) fn read_text_at(
    table: &dyn TableContext,
    context: &TableContextInfo,
    row_values: &[Option<TableRowColumnValue>],
    column_idx: usize,
    text: &mut TextReader,
) -> Option<String> {
    let descriptor = context.columns().get(column_idx)?;
    let value = row_values.get(column_idx)?.as_ref()?;
    let read = table.read_column(value, descriptor.prop_type()).ok()?;
    text.text(&read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_reader_starts_under_the_windows_1252_fallback_with_nothing_counted() {
        let reader = TextReader::new();
        assert_eq!(reader.codepage, 1252);
        assert_eq!(reader.stats, TextStats::default());
    }

    #[test]
    fn the_message_codepage_wins_then_the_internet_codepage_then_the_fallback() {
        let mut reader = TextReader::new();
        reader.use_message_codepage(Some(65001), Some(28591));
        assert_eq!(reader.codepage, 65001);
        reader.use_message_codepage(Some(0), Some(28591));
        assert_eq!(reader.codepage, 28591);
        reader.use_message_codepage(None, Some(-1));
        assert_eq!(reader.codepage, 1252);
    }

    #[test]
    fn integer32_reads_only_a_32_bit_integer() {
        assert_eq!(integer32(Some(&PropertyValue::Integer32(7))), Some(7));
        assert_eq!(integer32(Some(&PropertyValue::Time(5))), None);
        assert_eq!(integer32(None), None);
    }

    #[test]
    fn stats_add_field_by_field() {
        let mut total = TextStats {
            string8_values: 1,
            string8_non_ascii: 2,
            string8_replaced_bytes: 3,
            string8_unsupported: 4,
        };
        total.add(&TextStats {
            string8_values: 10,
            string8_non_ascii: 20,
            string8_replaced_bytes: 30,
            string8_unsupported: 40,
        });
        assert_eq!(
            total,
            TextStats {
                string8_values: 11,
                string8_non_ascii: 22,
                string8_replaced_bytes: 33,
                string8_unsupported: 44,
            }
        );
    }
}
