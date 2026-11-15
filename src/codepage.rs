//! Code page numbers to decoders (plan decision 16; ADR "Decode legacy code pages with
//! `encoding_rs`").
//!
//! This is the one module that names an `encoding_rs` type. [`decode`] turns bytes under a
//! Microsoft code page number into text and reports exactly how many bytes could not be decoded;
//! [`Decoding`] is the record of which pages a body was decoded under (decision 17), kept by the
//! callers and written to `metadata.json` as the `body.decoding` object.
//!
//! What is and is not here:
//!
//! - The table maps a Microsoft code page identifier to the `encoding_rs` encoding the WHATWG
//!   Encoding Standard names for it. Four pages are deliberately not in it, because
//!   `oxmsg_decode` decodes them in-house and verified: 1252, 28591 (ISO-8859-1), 20127
//!   (US-ASCII) and 65001 (UTF-8). The in-house Windows-1252 decoder stays because the standard's
//!   windows-1252 maps the five bytes Microsoft leaves undefined (0x81, 0x8D, 0x8F, 0x90, 0x9D)
//!   to C1 controls, where `tsp` replaces and counts them.
//! - The standard differs from Microsoft in places. Recorded deviations: 28599 (ISO-8859-9) is
//!   decoded as windows-1254, as the standard says, so 0x80 is the euro sign and not U+0080;
//!   the multibyte pages (932, 936, 949, 950) are the standard's Shift_JIS, GBK, EUC-KR and Big5,
//!   which are supersets of Microsoft's.
//! - A page that is not in the table has no decoder: [`decode`] returns `None` and the caller
//!   counts the bytes as lost. Nothing is guessed. Pages known to be absent: the OEM pages (437,
//!   850, 852, ...), the EBCDIC pages, Johab (1361), UTF-7, and the ISO-2022 family.
//! - No byte order mark is looked at: the bytes are decoded as the page says, so a stray
//!   `EF BB BF` under Windows-1256 is three characters, not a hidden switch to UTF-8.
//! - Malformed input never stops the decode. Each malformed sequence becomes one U+FFFD in the
//!   text and its length in bytes is added to `replaced_bytes`, so the count means bytes, as the
//!   loss reason `undecodable_bytes_replaced` does.

use std::collections::BTreeSet;

use encoding_rs::{
    BIG5, DecoderResult, EUC_JP, EUC_KR, Encoding, GB18030, GBK, IBM866, ISO_8859_2, ISO_8859_3,
    ISO_8859_4, ISO_8859_5, ISO_8859_6, ISO_8859_7, ISO_8859_8, ISO_8859_8_I, ISO_8859_13,
    ISO_8859_15, KOI8_R, KOI8_U, MACINTOSH, SHIFT_JIS, WINDOWS_874, WINDOWS_1250, WINDOWS_1251,
    WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, WINDOWS_1257, WINDOWS_1258,
};

/// Every code page number [`encoding_for`] maps, in ascending order. A test keeps this list and
/// the table in step; the robustness tests use it to exercise every decoder.
#[cfg(test)]
pub(crate) const MAPPED_CODEPAGES: &[u32] = &[
    866, 874, 932, 936, 949, 950, 1250, 1251, 1253, 1254, 1255, 1256, 1257, 1258, 10000, 20866,
    21866, 28592, 28593, 28594, 28595, 28596, 28597, 28598, 28599, 28603, 28605, 38598, 51932,
    51949, 54936,
];

/// The `encoding_rs` encoding for a Microsoft code page number, or `None` for a page with no
/// decoder here (including the four decoded in-house; see the module comment).
fn encoding_for(codepage: u32) -> Option<&'static Encoding> {
    Some(match codepage {
        866 => IBM866,
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        10000 => MACINTOSH,
        20866 => KOI8_R,
        21866 => KOI8_U,
        28592 => ISO_8859_2,
        28593 => ISO_8859_3,
        28594 => ISO_8859_4,
        28595 => ISO_8859_5,
        28596 => ISO_8859_6,
        28597 => ISO_8859_7,
        28598 => ISO_8859_8,
        // The standard decodes the label iso-8859-9 as windows-1254 (recorded deviation).
        28599 => WINDOWS_1254,
        28603 => ISO_8859_13,
        28605 => ISO_8859_15,
        38598 => ISO_8859_8_I,
        51932 => EUC_JP,
        51949 => EUC_KR,
        54936 => GB18030,
        _ => return None,
    })
}

/// True for a mapped code page whose characters can be more than one byte long. Under such a
/// page an ASCII-range byte can be the second byte of a character (the trail bytes of Shift_JIS,
/// GBK and Big5 start at 0x40), so a caller that splits a byte stream into runs must not treat
/// such a byte as plain ASCII while a run is open.
pub(crate) fn is_multibyte(codepage: u32) -> bool {
    matches!(codepage, 932 | 936 | 949 | 950 | 51932 | 51949 | 54936)
}

/// Text decoded under a code page, and how many input bytes were malformed or unmapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decoded {
    pub(crate) text: String,
    /// Bytes in malformed sequences. The text has one U+FFFD for each such sequence.
    pub(crate) replaced_bytes: u64,
}

/// Decodes `bytes` under the Microsoft code page `codepage`, or `None` when the page has no
/// decoder here. See the module comment for what that means and for what is counted.
pub(crate) fn decode(codepage: u32, bytes: &[u8]) -> Option<Decoded> {
    let encoding = encoding_for(codepage)?;
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let mut text = String::new();
    let mut replaced_bytes = 0u64;
    let mut position = 0usize;
    let mut stalls = 0u8;
    loop {
        let rest = &bytes[position..];
        // The decoder writes only into the capacity the string already has, so reserve what the
        // rest of the input can need, plus room for the replacement character pushed below.
        let needed = decoder
            .max_utf8_buffer_length_without_replacement(rest.len())
            .unwrap_or_else(|| rest.len().saturating_mul(3));
        text.reserve(needed.saturating_add(4));
        let (result, read) = decoder.decode_to_string_without_replacement(rest, &mut text, true);
        position += read;
        match result {
            DecoderResult::InputEmpty => break,
            DecoderResult::OutputFull => {
                if read == 0 {
                    stalls += 1;
                    if stalls > 2 {
                        // No progress after reserving: not expected. Count what is left as
                        // lost rather than loop.
                        if position < bytes.len() {
                            replaced_bytes += (bytes.len() - position) as u64;
                            text.push('\u{FFFD}');
                        }
                        break;
                    }
                } else {
                    stalls = 0;
                }
            }
            DecoderResult::Malformed(length, _) => {
                replaced_bytes += u64::from(length);
                text.push('\u{FFFD}');
                stalls = 0;
            }
        }
    }
    Some(Decoded {
        text,
        replaced_bytes,
    })
}

/// The code pages a body was decoded under (the `body.decoding` object of `metadata.json`,
/// plan decision 17).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Decoding {
    /// Pages under which at least one byte sequence became a non-ASCII character (other than a
    /// replacement character). UTF-8 is 65001, which is also what a native HTML body with no
    /// stored code page is read as.
    pub(crate) code_pages: BTreeSet<u32>,
    /// Pages under which at least one byte was replaced by U+FFFD, including pages with no
    /// decoder.
    pub(crate) replaced_code_pages: BTreeSet<u32>,
}

impl Decoding {
    /// Records one decode: `text` is what `codepage` produced from the bytes and
    /// `replaced_bytes` how many of them were replaced.
    pub(crate) fn note(&mut self, codepage: u32, text: &str, replaced_bytes: u64) {
        if replaced_bytes > 0 {
            self.replaced_code_pages.insert(codepage);
        }
        if text.chars().any(|c| !c.is_ascii() && c != '\u{FFFD}') {
            self.code_pages.insert(codepage);
        }
    }

    /// Records bytes lost under a page that has no decoder.
    pub(crate) fn note_replaced(&mut self, codepage: u32) {
        self.replaced_code_pages.insert(codepage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One row per table entry: the page, input bytes, and the text they must decode to. The
    /// multibyte rows include 950, whose second byte (0x40) is in the ASCII range.
    const KNOWN: &[(u32, &[u8], &str)] = &[
        (866, &[0x80], "\u{410}"),
        (874, &[0xA1], "\u{E01}"),
        (932, &[0x82, 0xA0], "\u{3042}"),
        (936, &[0xC4, 0xE3], "\u{4F60}"),
        (949, &[0xB0, 0xA1], "\u{AC00}"),
        (950, &[0xA4, 0x40], "\u{4E00}"),
        (1250, &[0xE8], "\u{10D}"),
        (1251, &[0xEF], "\u{43F}"),
        (1253, &[0xE1], "\u{3B1}"),
        (1254, &[0xF0], "\u{11F}"),
        (1255, &[0xE0], "\u{5D0}"),
        (1256, &[0xC7], "\u{627}"),
        (1257, &[0xE0], "\u{105}"),
        (1258, &[0xE0], "\u{E0}"),
        (10000, &[0x8E], "\u{E9}"),
        (20866, &[0xC1], "\u{430}"),
        (21866, &[0xA4], "\u{454}"),
        (28592, &[0xE8], "\u{10D}"),
        (28593, &[0xB1], "\u{127}"),
        (28594, &[0xE0], "\u{101}"),
        (28595, &[0xD0], "\u{430}"),
        (28596, &[0xC7], "\u{627}"),
        (28597, &[0xE1], "\u{3B1}"),
        (28598, &[0xE0], "\u{5D0}"),
        (28599, &[0xF0], "\u{11F}"),
        (28603, &[0xE0], "\u{105}"),
        (28605, &[0xA4], "\u{20AC}"),
        (38598, &[0xE0], "\u{5D0}"),
        (51932, &[0xA4, 0xA2], "\u{3042}"),
        (51949, &[0xB0, 0xA1], "\u{AC00}"),
        (54936, &[0xC4, 0xE3], "\u{4F60}"),
    ];

    #[test]
    fn every_mapped_page_decodes_its_known_sequence() {
        for &(page, bytes, expected) in KNOWN {
            let decoded = decode(page, bytes).unwrap_or_else(|| panic!("page {page} is mapped"));
            assert_eq!(decoded.text, expected, "page {page}");
            assert_eq!(decoded.replaced_bytes, 0, "page {page}");
        }
    }

    #[test]
    fn the_list_the_table_and_the_known_rows_agree() {
        for page in 0..=70_000u32 {
            assert_eq!(
                encoding_for(page).is_some(),
                MAPPED_CODEPAGES.contains(&page),
                "page {page}"
            );
        }
        let known: Vec<u32> = KNOWN.iter().map(|&(page, _, _)| page).collect();
        assert_eq!(known, MAPPED_CODEPAGES);
    }

    #[test]
    fn pages_with_no_decoder_here_return_none() {
        // In-house pages (1252, 28591, 20127, 65001), OEM and EBCDIC pages, Johab, and nonsense.
        for page in [
            0u32, 37, 437, 500, 708, 737, 775, 850, 852, 855, 857, 860, 861, 862, 863, 865, 869,
            1252, 1361, 20127, 28591, 50220, 65000, 65001, 99_999,
        ] {
            assert!(decode(page, b"abc").is_none(), "page {page}");
        }
    }

    #[test]
    fn pages_the_standard_lacks_have_no_label_in_it() {
        // The table is written from Microsoft's list and the standard's: a page the standard has
        // no label for cannot appear in it. If one of these gains a label, the table can grow.
        for label in [
            "ibm437", "cp437", "ibm850", "cp850", "ibm852", "cp852", "johab", "utf-7", "ibm037",
        ] {
            assert!(Encoding::for_label(label.as_bytes()).is_none(), "{label}");
        }
    }

    #[test]
    fn every_mapped_page_passes_ascii_text_through_unchanged() {
        for &page in MAPPED_CODEPAGES {
            let decoded = decode(page, b"IPM.Note <p>x</p>").expect("mapped");
            assert_eq!(decoded.text, "IPM.Note <p>x</p>", "page {page}");
            assert_eq!(decoded.replaced_bytes, 0, "page {page}");
        }
    }

    #[test]
    fn iso_8859_9_follows_the_standard_and_not_microsoft_for_0x80() {
        // Recorded deviation: windows-1254 puts the euro sign at 0x80; ISO-8859-9 has U+0080.
        assert_eq!(decode(28599, &[0x80]).expect("mapped").text, "\u{20AC}");
    }

    #[test]
    fn a_byte_order_mark_is_decoded_as_bytes_not_obeyed() {
        let decoded = decode(1256, &[0xEF, 0xBB, 0xBF, b'a']).expect("mapped");
        assert_eq!(decoded.text.chars().count(), 4);
        assert!(decoded.text.ends_with('a'));
        assert_eq!(decoded.replaced_bytes, 0);
    }

    #[test]
    fn a_malformed_sequence_counts_its_bytes_and_writes_one_replacement() {
        // A Shift_JIS lead byte at the end of the input.
        let decoded = decode(932, &[b'a', 0x82]).expect("mapped");
        assert_eq!(decoded.text, "a\u{FFFD}");
        assert_eq!(decoded.replaced_bytes, 1);
        // A lead byte followed by a byte that cannot be a trail byte: the lead is lost, the
        // following byte is read again as ASCII.
        let decoded = decode(932, &[0x82, b' ', b'x']).expect("mapped");
        assert_eq!(decoded.text, "\u{FFFD} x");
        assert_eq!(decoded.replaced_bytes, 1);
    }

    #[test]
    fn empty_input_decodes_to_empty_text_under_a_mapped_page() {
        let decoded = decode(1256, &[]).expect("mapped");
        assert_eq!(decoded.text, "");
        assert_eq!(decoded.replaced_bytes, 0);
    }

    #[test]
    fn multibyte_pages_are_the_ones_where_a_character_can_span_bytes() {
        for page in [932u32, 936, 949, 950, 51932, 51949, 54936] {
            assert!(is_multibyte(page), "page {page}");
        }
        for page in [866u32, 874, 1250, 1256, 28599, 1252, 65001, 437] {
            assert!(!is_multibyte(page), "page {page}");
        }
    }

    #[test]
    fn decoding_records_pages_by_what_was_decoded_and_what_was_replaced() {
        let mut decoding = Decoding::default();
        decoding.note(1252, "plain ascii", 0);
        assert!(decoding.code_pages.is_empty());
        assert!(decoding.replaced_code_pages.is_empty());

        decoding.note(1256, "\u{627}", 0);
        decoding.note(1252, "a\u{FFFD}", 1);
        decoding.note_replaced(437);
        let code_pages: Vec<u32> = decoding.code_pages.iter().copied().collect();
        let replaced: Vec<u32> = decoding.replaced_code_pages.iter().copied().collect();
        assert_eq!(code_pages, vec![1256]);
        assert_eq!(replaced, vec![437, 1252]);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_decode_without_panic_under_every_mapped_page(
            bytes in prop::collection::vec(any::<u8>(), 0..300),
        ) {
            for &page in MAPPED_CODEPAGES {
                let decoded = decode(page, &bytes).expect("mapped");
                prop_assert!(decoded.replaced_bytes <= bytes.len() as u64);
            }
        }
    }
}
