//! De-encapsulation differential verification (`--verify-deencap`, M4e-1): the HTML the in-house
//! MS-OXRTFEX de-encapsulation recovers, compared with `msg_parser`'s `html_from_rtf()` for the
//! same `.msg` files.
//!
//! `msg_parser` is a weak oracle here: its scanner treats `\'hh` escapes as Latin-1 whatever the
//! code page, does not honour the 10-token recognition rule (it returned HTML for a genuinely
//! RTF-authored message in M2), and reads RTF decompressed with its own dictionary (one corpus
//! file differs by one byte). So agreement is graded, from byte-identical down to different, and
//! every disagreement is a finding to triage against the specification, not an error in either
//! path. Like the other verify modes this reads real content internally and prints only counts,
//! flags, code page numbers, and byte-size differences, never the values compared.
//!
//! It also compares the recognition rule with the whole-document `\fromhtml1` search the default
//! report uses (`rtf_bytes_contain_fromhtml`), which is the M4e-1 item "confirm that the 10-token
//! rule matches `check_compressed_rtf_bytes`".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use msg_parser::Outlook;

use crate::oxmsg_decode::{expected_variable_stream_path, read_stream_bytes};
use crate::rtf_deencap::{Diagnostics, Limits, RtfKind, deencapsulate_html, recognize};
use crate::shared::{CompoundFile, PROP_RTF_COMPRESSED, rtf_bytes_contain_fromhtml};
use crate::verify::{BoolFieldTally, compare_bool_field, print_bool_field_tally};

/// How closely two HTML strings agree, from most to least.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HtmlAgreement {
    Exact,
    EqualIgnoringWhitespace,
    EqualIgnoringWhitespaceAndNonAscii,
    /// The text outside tags is the same ignoring whitespace; the tags differ.
    SameVisibleText,
    SameVisibleTextAsciiOnly,
    Different,
}

pub(crate) fn strip_whitespace(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub(crate) fn ascii_only(text: &str) -> String {
    text.chars().filter(char::is_ascii).collect()
}

/// The text outside `<...>` spans, without whitespace. A heuristic for grading agreement, not an
/// HTML parser.
pub(crate) fn visible_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c if c.is_whitespace() => {}
            c => text.push(c),
        }
    }
    text
}

pub(crate) fn classify_agreement(oracle: &str, custom: &str) -> HtmlAgreement {
    if oracle == custom {
        return HtmlAgreement::Exact;
    }
    let (oracle_compact, custom_compact) = (strip_whitespace(oracle), strip_whitespace(custom));
    if oracle_compact == custom_compact {
        return HtmlAgreement::EqualIgnoringWhitespace;
    }
    if ascii_only(&oracle_compact) == ascii_only(&custom_compact) {
        return HtmlAgreement::EqualIgnoringWhitespaceAndNonAscii;
    }
    let (oracle_text, custom_text) = (visible_text(oracle), visible_text(custom));
    if oracle_text == custom_text {
        return HtmlAgreement::SameVisibleText;
    }
    if ascii_only(&oracle_text) == ascii_only(&custom_text) {
        return HtmlAgreement::SameVisibleTextAsciiOnly;
    }
    HtmlAgreement::Different
}

#[derive(Default)]
pub(crate) struct AgreementTally {
    exact: u64,
    equal_ignoring_whitespace: u64,
    equal_ignoring_whitespace_and_non_ascii: u64,
    same_visible_text: u64,
    same_visible_text_ascii_only: u64,
    different: u64,
}

impl AgreementTally {
    pub(crate) fn record(&mut self, agreement: HtmlAgreement) {
        match agreement {
            HtmlAgreement::Exact => self.exact += 1,
            HtmlAgreement::EqualIgnoringWhitespace => self.equal_ignoring_whitespace += 1,
            HtmlAgreement::EqualIgnoringWhitespaceAndNonAscii => {
                self.equal_ignoring_whitespace_and_non_ascii += 1;
            }
            HtmlAgreement::SameVisibleText => self.same_visible_text += 1,
            HtmlAgreement::SameVisibleTextAsciiOnly => self.same_visible_text_ascii_only += 1,
            HtmlAgreement::Different => self.different += 1,
        }
    }
}

/// What reading the message's compressed RTF gave.
enum RtfRead {
    Absent,
    DecompressionFailed,
    Bytes(Vec<u8>),
}

/// Reads and decompresses `PidTagRtfCompressed` (MS-OXRTFCP, via `compressed-rtf`), the same
/// stream the default report reads.
fn read_decompressed_rtf(comp: &mut CompoundFile) -> RtfRead {
    let Some(compressed) = read_stream_bytes(
        comp,
        &expected_variable_stream_path(Path::new("/"), PROP_RTF_COMPRESSED, 0x0102),
    )
    .filter(|bytes| !bytes.is_empty()) else {
        return RtfRead::Absent;
    };
    // `compressed_rtf::decompress_rtf` indexes the first 16 bytes unconditionally.
    if compressed.len() < 16 {
        return RtfRead::DecompressionFailed;
    }
    match compressed_rtf::decompress_rtf(&compressed) {
        Ok(rtf) => RtfRead::Bytes(rtf.as_bytes().to_vec()),
        Err(_) => RtfRead::DecompressionFailed,
    }
}

#[derive(Default)]
struct DiagnosticsTotals {
    htmltag_groups: u64,
    ignorable_destinations_skipped: u64,
    standard_destinations_skipped: u64,
    htmlrtf_regions: u64,
    hex_escapes: u64,
    bad_hex_escapes: u64,
    unicode_escapes: u64,
    unpaired_surrogates: u64,
    paragraph_breaks: u64,
    tabs: u64,
    control_symbols_ignored: u64,
    binary_bytes_skipped: u64,
    object_placeholders: u64,
    undecodable_bytes: u64,
    fonts_defined: u64,
    stray_group_ends: u64,
    files_with_unclosed_groups: u64,
    files_depth_limit_hit: u64,
    files_output_limit_hit: u64,
    files_meta_charset_declared: u64,
    files_default_codepage_declared: u64,
    default_codepage_files: BTreeMap<u32, u64>,
    unsupported_codepage_files: BTreeMap<u32, u64>,
}

impl DiagnosticsTotals {
    fn add(&mut self, d: &Diagnostics) {
        self.htmltag_groups += d.htmltag_groups;
        self.ignorable_destinations_skipped += d.ignorable_destinations_skipped;
        self.standard_destinations_skipped += d.standard_destinations_skipped;
        self.htmlrtf_regions += d.htmlrtf_regions;
        self.hex_escapes += d.hex_escapes;
        self.bad_hex_escapes += d.bad_hex_escapes;
        self.unicode_escapes += d.unicode_escapes;
        self.unpaired_surrogates += d.unpaired_surrogates;
        self.paragraph_breaks += d.paragraph_breaks;
        self.tabs += d.tabs;
        self.control_symbols_ignored += d.control_symbols_ignored;
        self.binary_bytes_skipped += d.binary_bytes_skipped;
        self.object_placeholders += d.object_placeholders;
        self.undecodable_bytes += d.undecodable_bytes;
        self.fonts_defined += d.fonts_defined;
        self.stray_group_ends += d.stray_group_ends;
        self.files_with_unclosed_groups += u64::from(d.unclosed_groups_at_end > 0);
        self.files_depth_limit_hit += u64::from(d.depth_limit_hit);
        self.files_output_limit_hit += u64::from(d.output_limit_hit);
        self.files_meta_charset_declared += u64::from(d.meta_charset_declared);
        self.files_default_codepage_declared += u64::from(d.default_codepage_declared);
        *self
            .default_codepage_files
            .entry(d.default_codepage)
            .or_insert(0) += 1;
        for codepage in &d.unsupported_codepages {
            *self
                .unsupported_codepage_files
                .entry(*codepage)
                .or_insert(0) += 1;
        }
    }
}

#[derive(Default)]
pub(crate) struct DeencapVerifyTotals {
    open_errors_msg_parser: u64,
    open_errors_custom: u64,
    rtf_absent: u64,
    rtf_decompression_failed: u64,
    kind_encapsulated_html: u64,
    kind_encapsulated_text: u64,
    kind_plain_rtf: u64,
    kind_not_rtf: u64,
    /// The 10-token recognition rule against the whole-document `\fromhtml1` search.
    recognition_vs_marker_search: BoolFieldTally,
    /// Files where the recognition rule and the marker search disagreed,
    /// keyed by `RtfKind::label()` (what the 10-token rule concluded).
    recognition_mismatch_by_kind: BTreeMap<&'static str, u64>,
    /// Whether `msg_parser` and the custom path each recovered HTML from the RTF.
    html_presence: BoolFieldTally,
    agreement: AgreementTally,
    /// Custom minus `msg_parser` HTML length in bytes, for each pair that is not byte-identical.
    html_byte_deltas: Vec<i64>,
    custom_html_bytes_total: u64,
    msg_parser_html_bytes_total: u64,
    diagnostics: DiagnosticsTotals,
}

fn record_kind(totals: &mut DeencapVerifyTotals, kind: RtfKind) {
    match kind {
        RtfKind::EncapsulatedHtml => totals.kind_encapsulated_html += 1,
        RtfKind::EncapsulatedText => totals.kind_encapsulated_text += 1,
        RtfKind::PlainRtf => totals.kind_plain_rtf += 1,
        RtfKind::NotRtf => totals.kind_not_rtf += 1,
    }
}

pub(crate) fn collect_deencap_verify_totals(files: &[PathBuf]) -> DeencapVerifyTotals {
    let mut totals = DeencapVerifyTotals::default();
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

        let oracle_html = outlook.html_from_rtf().filter(|html| !html.is_empty());

        let mut custom_html: Option<String> = None;
        match read_decompressed_rtf(&mut comp) {
            RtfRead::Absent => totals.rtf_absent += 1,
            RtfRead::DecompressionFailed => totals.rtf_decompression_failed += 1,
            RtfRead::Bytes(rtf) => {
                let kind = recognize(&rtf);
                record_kind(&mut totals, kind);

                let rule_says_html = kind == RtfKind::EncapsulatedHtml;
                let marker_says_html = rtf_bytes_contain_fromhtml(&rtf);
                if rule_says_html != marker_says_html {
                    *totals
                        .recognition_mismatch_by_kind
                        .entry(kind.label())
                        .or_insert(0) += 1;
                }
                totals
                    .recognition_vs_marker_search
                    .record(compare_bool_field(marker_says_html, rule_says_html));

                if let Ok(result) = deencapsulate_html(&rtf, Limits::default()) {
                    totals.diagnostics.add(&result.diagnostics);
                    custom_html = Some(result.html);
                }
            }
        }

        totals.html_presence.record(compare_bool_field(
            oracle_html.is_some(),
            custom_html.as_ref().is_some_and(|html| !html.is_empty()),
        ));
        totals.custom_html_bytes_total += custom_html.as_ref().map_or(0, |h| h.len() as u64);
        totals.msg_parser_html_bytes_total += oracle_html.as_ref().map_or(0, |h| h.len() as u64);

        if let (Some(oracle), Some(custom)) = (&oracle_html, &custom_html) {
            let agreement = classify_agreement(oracle, custom);
            totals.agreement.record(agreement);
            if agreement != HtmlAgreement::Exact {
                totals
                    .html_byte_deltas
                    .push(custom.len() as i64 - oracle.len() as i64);
            }
        }
    }
    totals
}

fn print_codepage_map(name: &str, map: &BTreeMap<u32, u64>) {
    for (codepage, files) in map {
        println!("{name}_{codepage}_files={files}");
    }
}

pub(crate) fn print_deencap_verify_report(totals: &DeencapVerifyTotals) {
    println!("open_errors_msg_parser={}", totals.open_errors_msg_parser);
    println!("open_errors_custom={}", totals.open_errors_custom);
    println!("rtf_absent={}", totals.rtf_absent);
    println!(
        "rtf_decompression_failed={}",
        totals.rtf_decompression_failed
    );
    println!(
        "rtf_kind_encapsulated_html={}",
        totals.kind_encapsulated_html
    );
    println!(
        "rtf_kind_encapsulated_text={}",
        totals.kind_encapsulated_text
    );
    println!("rtf_kind_plain_rtf={}", totals.kind_plain_rtf);
    println!("rtf_kind_not_rtf={}", totals.kind_not_rtf);
    print_bool_field_tally(
        "recognition_vs_marker_search",
        &totals.recognition_vs_marker_search,
    );
    print_bool_field_tally("html_presence", &totals.html_presence);
    print_bool_field_tally(
        "recognition_vs_marker_search",
        &totals.recognition_vs_marker_search,
    );
    for (label, files) in &totals.recognition_mismatch_by_kind {
        println!("recognition_mismatch_kind_{label}_files={files}");
    }
    let a = &totals.agreement;
    println!("html_agreement_exact={}", a.exact);
    println!(
        "html_agreement_equal_ignoring_whitespace={}",
        a.equal_ignoring_whitespace
    );
    println!(
        "html_agreement_equal_ignoring_whitespace_and_non_ascii={}",
        a.equal_ignoring_whitespace_and_non_ascii
    );
    println!("html_agreement_same_visible_text={}", a.same_visible_text);
    println!(
        "html_agreement_same_visible_text_ascii_only={}",
        a.same_visible_text_ascii_only
    );
    println!("html_agreement_different={}", a.different);
    for delta in &totals.html_byte_deltas {
        println!("html_bytes_delta={delta}");
    }
    println!("custom_html_bytes_total={}", totals.custom_html_bytes_total);
    println!(
        "msg_parser_html_bytes_total={}",
        totals.msg_parser_html_bytes_total
    );

    let d = &totals.diagnostics;
    println!("deencap_htmltag_groups_total={}", d.htmltag_groups);
    println!(
        "deencap_ignorable_destinations_skipped_total={}",
        d.ignorable_destinations_skipped
    );
    println!(
        "deencap_standard_destinations_skipped_total={}",
        d.standard_destinations_skipped
    );
    println!("deencap_htmlrtf_regions_total={}", d.htmlrtf_regions);
    println!("deencap_hex_escapes_total={}", d.hex_escapes);
    println!("deencap_bad_hex_escapes_total={}", d.bad_hex_escapes);
    println!("deencap_unicode_escapes_total={}", d.unicode_escapes);
    println!(
        "deencap_unpaired_surrogates_total={}",
        d.unpaired_surrogates
    );
    println!("deencap_paragraph_breaks_total={}", d.paragraph_breaks);
    println!("deencap_tabs_total={}", d.tabs);
    println!(
        "deencap_control_symbols_ignored_total={}",
        d.control_symbols_ignored
    );
    println!(
        "deencap_binary_bytes_skipped_total={}",
        d.binary_bytes_skipped
    );
    println!(
        "deencap_object_placeholders_total={}",
        d.object_placeholders
    );
    println!("deencap_undecodable_bytes_total={}", d.undecodable_bytes);
    println!("deencap_fonts_defined_total={}", d.fonts_defined);
    println!("deencap_stray_group_ends_total={}", d.stray_group_ends);
    println!(
        "deencap_files_with_unclosed_groups={}",
        d.files_with_unclosed_groups
    );
    println!("deencap_files_depth_limit_hit={}", d.files_depth_limit_hit);
    println!(
        "deencap_files_output_limit_hit={}",
        d.files_output_limit_hit
    );
    println!(
        "deencap_files_meta_charset_declared={}",
        d.files_meta_charset_declared
    );
    println!(
        "deencap_files_default_codepage_declared={}",
        d.files_default_codepage_declared
    );
    print_codepage_map("deencap_default_codepage", &d.default_codepage_files);
    print_codepage_map(
        "deencap_unsupported_codepage",
        &d.unsupported_codepage_files,
    );
}

pub(crate) fn run_deencap_verify(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    let totals = collect_deencap_verify_totals(files);
    println!("inventory=privacy_safe");
    println!("input_kind=msg_deencap_verify");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");
    print_deencap_verify_report(&totals);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_is_graded_from_exact_downward() {
        assert_eq!(
            classify_agreement("<p>a</p>", "<p>a</p>"),
            HtmlAgreement::Exact
        );
        assert_eq!(
            classify_agreement("<p>a b</p>\r\n", "<p>a b</p>"),
            HtmlAgreement::EqualIgnoringWhitespace
        );
        assert_eq!(
            classify_agreement("<p>caf\u{e9}</p>", "<p>caf\u{FFFD}</p>"),
            HtmlAgreement::EqualIgnoringWhitespaceAndNonAscii
        );
        assert_eq!(
            classify_agreement("<p>a</p>", "<div>a</div>"),
            HtmlAgreement::SameVisibleText
        );
        assert_eq!(
            classify_agreement("<p>caf\u{e9}</p>", "<div>caf\u{FFFD}</div>"),
            HtmlAgreement::SameVisibleTextAsciiOnly
        );
        assert_eq!(
            classify_agreement("<p>a</p>", "<p>b</p>"),
            HtmlAgreement::Different
        );
    }

    #[test]
    fn visible_text_drops_tags_and_whitespace() {
        assert_eq!(
            visible_text("<p class=x>Hello <b>big</b>\r\nworld</p>"),
            "Hellobigworld"
        );
        assert_eq!(visible_text("no tags"), "notags");
        // An unterminated tag swallows the rest; it is a grading heuristic, not a parser.
        assert_eq!(visible_text("a<b"), "a");
    }

    #[test]
    fn helpers_strip_as_described() {
        assert_eq!(strip_whitespace(" a\tb\r\nc "), "abc");
        assert_eq!(ascii_only("a\u{e9}b"), "ab");
    }

    #[test]
    fn the_tally_counts_each_grade() {
        let mut tally = AgreementTally::default();
        tally.record(HtmlAgreement::Exact);
        tally.record(HtmlAgreement::Exact);
        tally.record(HtmlAgreement::Different);
        assert_eq!((tally.exact, tally.different), (2, 1));
    }
}
