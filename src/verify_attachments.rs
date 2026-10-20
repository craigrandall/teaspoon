//! Attachment payload differential (`--verify-attachments`, M4f-2c): for each top-level by-value
//! attachment, the SHA-256 of the bytes the export would write (read through the export's own
//! reader, [`MsgAttachments::open_data`], in chunks) against the SHA-256 of `msg_parser`'s
//! `payload_bytes` for the same attachment.
//!
//! Reads real content internally and prints only counts. Attachments are paired by position in
//! storage order, because the export names and records attachments by that position; a
//! diagnostic multiset comparison (order ignored) is printed beside it, so a difference of order
//! can be told from a difference of content.
//!
//! What it does and does not check: it compares the bytes of by-value attachments of top-level
//! messages. It does not open embedded messages (`msg_parser` does not either), and it says
//! nothing about names, content IDs, or the files the export wrote (the export's own read-back
//! covers those against the manifest). `msg_parser` reports an empty payload both for a
//! zero-byte attachment and for one whose data stream is missing, so a custom-path missing
//! stream against an empty `msg_parser` payload is counted as agreed, not as a match.

use std::io::Read;
use std::path::PathBuf;

use anyhow::Result;
use msg_parser::Outlook;
use sha2::{Digest, Sha256};

use crate::model::MessageContent;
use crate::msg_report::MSG_ATTACH_METHOD_BY_VALUE;
use crate::shared::ATTACH_METHOD_BY_VALUE;
use crate::source_msg::{MsgAttachments, OpenMessage, open_message};
use crate::stage::COPY_CHUNK_BYTES;
use crate::verify::{CountComparison, CountTally, print_count_tally};

type Digest32 = [u8; 32];

fn digest_of(bytes: &[u8]) -> Digest32 {
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&Sha256::digest(bytes));
    digest
}

/// What the export's reader gave for one by-value attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CustomPayload {
    /// The whole stream was read; the length is what was read and agrees with the declared one.
    Hashed { len: u64, digest: Digest32 },
    /// There is no data stream that can be opened (the export records `data_stream_missing`).
    Missing,
    /// A read failed, or the bytes read were not the declared length (the export records
    /// `data_read_failed` or `data_length_mismatch`).
    Failed,
}

/// Reads `source` to the end in chunks and hashes it, as the export's stager does.
pub(crate) fn hash_stream(source: &mut dyn Read, declared: Option<u64>) -> CustomPayload {
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
                total += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return CustomPayload::Failed,
        }
    }
    if declared.is_some_and(|declared| declared != total) {
        return CustomPayload::Failed;
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hasher.finalize());
    CustomPayload::Hashed { len: total, digest }
}

fn hash_custom(handle: &mut MsgAttachments, index: usize) -> CustomPayload {
    match handle.open_data(index) {
        None => CustomPayload::Missing,
        Some((mut stream, declared)) => hash_stream(&mut stream, Some(declared)),
    }
}

/// One top-level attachment as the custom path sees it. `payload` is set only for a by-value
/// attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CustomSide {
    pub(crate) payload: Option<CustomPayload>,
}

/// One top-level attachment as `msg_parser` sees it. `len` and `digest` are meaningful only for a
/// by-value attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParserSide {
    pub(crate) by_value: bool,
    pub(crate) len: u64,
    pub(crate) digest: Digest32,
}

/// The custom path's view of every top-level attachment of an open message, in storage order.
pub(crate) fn custom_sides(
    content: &MessageContent,
    handle: &mut MsgAttachments,
) -> Vec<CustomSide> {
    content
        .attachments
        .iter()
        .enumerate()
        .map(|(index, info)| CustomSide {
            payload: (info.method == Some(ATTACH_METHOD_BY_VALUE))
                .then(|| hash_custom(handle, index)),
        })
        .collect()
}

fn parser_sides(outlook: &Outlook) -> Vec<ParserSide> {
    outlook
        .attachments
        .iter()
        .map(|attach| {
            let by_value = attach.attach_method == MSG_ATTACH_METHOD_BY_VALUE;
            ParserSide {
                by_value,
                len: if by_value {
                    attach.payload_bytes.len() as u64
                } else {
                    0
                },
                digest: if by_value {
                    digest_of(&attach.payload_bytes)
                } else {
                    [0u8; 32]
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadComparison {
    /// Same length and same SHA-256.
    Match,
    /// Both sides produced bytes, and they differ.
    Mismatch,
    /// The custom path has no data stream and `msg_parser`'s payload is empty: the two agree
    /// that there is nothing to extract (not a match: the empty payload is ambiguous).
    MissingAgreed,
    /// The custom path has no data stream but `msg_parser` found bytes.
    MissingCustomOnly,
    /// The custom path failed to read the stream.
    CustomFailed,
}

pub(crate) fn compare_payload(
    custom: CustomPayload,
    parser_len: u64,
    parser_digest: Digest32,
) -> PayloadComparison {
    match custom {
        CustomPayload::Hashed { len, digest } => {
            if len == parser_len && digest == parser_digest {
                PayloadComparison::Match
            } else {
                PayloadComparison::Mismatch
            }
        }
        CustomPayload::Missing => {
            if parser_len == 0 {
                PayloadComparison::MissingAgreed
            } else {
                PayloadComparison::MissingCustomOnly
            }
        }
        CustomPayload::Failed => PayloadComparison::CustomFailed,
    }
}

#[derive(Default)]
pub(crate) struct PayloadTotals {
    pub(crate) open_errors_msg_parser: u64,
    pub(crate) open_errors_custom: u64,
    pub(crate) messages: u64,
    pub(crate) attachments_custom: u64,
    pub(crate) attachments_msg_parser: u64,
    /// Messages whose two attachment lists have different lengths.
    pub(crate) count_mismatch_messages: u64,
    /// By-value pairs compared by position: match (same length and SHA-256) or mismatch.
    pub(crate) payload: CountTally,
    pub(crate) missing_agreed: u64,
    pub(crate) missing_custom_only: u64,
    pub(crate) custom_failed: u64,
    /// Positions where exactly one side calls the attachment by value.
    pub(crate) method_disagreement: u64,
    pub(crate) bytes_matched: u64,
    pub(crate) largest_payload_matched: u64,
    /// Per message, the by-value (length, SHA-256) lists compared with order ignored.
    pub(crate) multiset: CountTally,
}

impl PayloadTotals {
    fn record(&mut self, comparison: PayloadComparison, len: u64) {
        match comparison {
            PayloadComparison::Match => {
                self.payload.record(CountComparison::Match);
                self.bytes_matched += len;
                self.largest_payload_matched = self.largest_payload_matched.max(len);
            }
            PayloadComparison::Mismatch => self.payload.record(CountComparison::Mismatch),
            PayloadComparison::MissingAgreed => self.missing_agreed += 1,
            PayloadComparison::MissingCustomOnly => self.missing_custom_only += 1,
            PayloadComparison::CustomFailed => self.custom_failed += 1,
        }
    }

    /// Everything that must be 0 on a clean corpus. The order-free comparison is not included:
    /// the export relies on position, so a difference of order is already a positional mismatch.
    pub(crate) fn gate_violations(&self) -> u64 {
        self.open_errors_msg_parser
            + self.open_errors_custom
            + self.count_mismatch_messages
            + self.payload.mismatched
            + self.missing_custom_only
            + self.custom_failed
            + self.method_disagreement
    }
}

/// Compares one message's attachments, pairing by position, and adds to `totals`.
pub(crate) fn compare_message(
    custom: &[CustomSide],
    parser: &[ParserSide],
    totals: &mut PayloadTotals,
) {
    totals.messages += 1;
    totals.attachments_custom += custom.len() as u64;
    totals.attachments_msg_parser += parser.len() as u64;
    if custom.len() != parser.len() {
        totals.count_mismatch_messages += 1;
    }
    for (c, p) in custom.iter().zip(parser) {
        match (c.payload, p.by_value) {
            (Some(payload), true) => {
                totals.record(compare_payload(payload, p.len, p.digest), p.len);
            }
            (None, false) => {}
            _ => totals.method_disagreement += 1,
        }
    }

    let mut custom_list: Vec<(u64, Digest32)> = Vec::new();
    let mut custom_ok = true;
    for c in custom {
        match c.payload {
            Some(CustomPayload::Hashed { len, digest }) => custom_list.push((len, digest)),
            Some(CustomPayload::Missing) => custom_list.push((0, digest_of(b""))),
            Some(CustomPayload::Failed) => custom_ok = false,
            None => {}
        }
    }
    let mut parser_list: Vec<(u64, Digest32)> = parser
        .iter()
        .filter(|p| p.by_value)
        .map(|p| (p.len, p.digest))
        .collect();
    custom_list.sort();
    parser_list.sort();
    totals
        .multiset
        .record(if custom_ok && custom_list == parser_list {
            CountComparison::Match
        } else {
            CountComparison::Mismatch
        });
}

/// Runs both readers over every file and accumulates the comparison.
pub(crate) fn collect_payload_totals(files: &[PathBuf]) -> PayloadTotals {
    let mut totals = PayloadTotals::default();
    for file in files {
        let outlook = match Outlook::from_path(file) {
            Ok(outlook) => outlook,
            Err(_) => {
                totals.open_errors_msg_parser += 1;
                continue;
            }
        };
        let OpenMessage {
            content,
            attachments: mut handle,
        } = match open_message(file) {
            Ok(open) => open,
            Err(_) => {
                totals.open_errors_custom += 1;
                continue;
            }
        };
        let custom = custom_sides(&content, &mut handle);
        let parser = parser_sides(&outlook);
        compare_message(&custom, &parser, &mut totals);
    }
    totals
}

/// Prints the counts. Every key is always printed.
pub(crate) fn print_payload_totals(totals: &PayloadTotals) {
    println!(
        "attachment_payload_open_errors_msg_parser={}",
        totals.open_errors_msg_parser
    );
    println!(
        "attachment_payload_open_errors_custom={}",
        totals.open_errors_custom
    );
    println!("attachment_payload_messages={}", totals.messages);
    println!(
        "attachment_payload_attachments_custom={}",
        totals.attachments_custom
    );
    println!(
        "attachment_payload_attachments_msg_parser={}",
        totals.attachments_msg_parser
    );
    println!(
        "attachment_payload_count_mismatch_messages={}",
        totals.count_mismatch_messages
    );
    print_count_tally("attachment_payload_sha256", &totals.payload);
    println!(
        "attachment_payload_missing_agreed={}",
        totals.missing_agreed
    );
    println!(
        "attachment_payload_missing_custom_only={}",
        totals.missing_custom_only
    );
    println!("attachment_payload_custom_failed={}", totals.custom_failed);
    println!(
        "attachment_payload_method_disagreement={}",
        totals.method_disagreement
    );
    println!("attachment_payload_bytes_matched={}", totals.bytes_matched);
    println!(
        "attachment_payload_largest_matched={}",
        totals.largest_payload_matched
    );
    print_count_tally("attachment_payload_multiset", &totals.multiset);
    println!(
        "attachment_payload_gate_violations={}",
        totals.gate_violations()
    );
}

/// `--verify-attachments`: counts only.
pub(crate) fn run_attachments_verify(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    let totals = collect_payload_totals(files);
    println!("inventory=privacy_safe");
    println!("input_kind=msg_verify_attachments");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");
    print_payload_totals(&totals);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use clap::Parser;
    use std::io::Write;
    use std::path::Path;

    // ---- readers that misbehave -------------------------------------------------------

    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(self.data.len()).min(buf.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    struct Breaks {
        good: usize,
    }

    impl Read for Breaks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.good == 0 {
                return Err(std::io::Error::other("broken"));
            }
            let n = self.good.min(buf.len());
            buf[..n].fill(7);
            self.good -= n;
            Ok(n)
        }
    }

    struct InterruptsOnce<'a> {
        data: &'a [u8],
        tripped: bool,
    }

    impl Read for InterruptsOnce<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.tripped {
                self.tripped = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.data.len().min(buf.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn hashed(bytes: &[u8]) -> CustomPayload {
        CustomPayload::Hashed {
            len: bytes.len() as u64,
            digest: digest_of(bytes),
        }
    }

    fn parser_side(bytes: &[u8]) -> ParserSide {
        ParserSide {
            by_value: true,
            len: bytes.len() as u64,
            digest: digest_of(bytes),
        }
    }

    fn not_by_value() -> ParserSide {
        ParserSide {
            by_value: false,
            len: 0,
            digest: [0u8; 32],
        }
    }

    fn by_value_side(payload: CustomPayload) -> CustomSide {
        CustomSide {
            payload: Some(payload),
        }
    }

    const OTHER_METHOD: CustomSide = CustomSide { payload: None };

    // ---- hashing ----------------------------------------------------------------------

    #[test]
    fn a_stream_is_hashed_in_chunks_like_the_whole_payload() {
        let payload = patterned(3 * COPY_CHUNK_BYTES + 17);
        let result = hash_stream(
            &mut std::io::Cursor::new(&payload),
            Some(payload.len() as u64),
        );
        assert_eq!(result, hashed(&payload));
    }

    #[test]
    fn short_reads_and_an_interrupt_do_not_change_the_digest() {
        let payload = patterned(5000);
        let trickled = hash_stream(
            &mut Trickle {
                data: &payload,
                step: 7,
            },
            None,
        );
        let interrupted = hash_stream(
            &mut InterruptsOnce {
                data: &payload,
                tripped: false,
            },
            Some(5000),
        );
        assert_eq!(trickled, hashed(&payload));
        assert_eq!(interrupted, hashed(&payload));
    }

    #[test]
    fn an_empty_stream_hashes_to_the_empty_digest() {
        assert_eq!(hash_stream(&mut std::io::empty(), Some(0)), hashed(b""));
    }

    #[test]
    fn a_read_error_or_a_wrong_length_is_a_failure_not_a_digest() {
        assert_eq!(
            hash_stream(&mut Breaks { good: 1000 }, Some(5000)),
            CustomPayload::Failed
        );
        let payload = patterned(11);
        assert_eq!(
            hash_stream(&mut std::io::Cursor::new(&payload), Some(10)),
            CustomPayload::Failed
        );
    }

    #[test]
    fn the_digest_function_matches_a_known_vector() {
        let abc = digest_of(b"abc");
        assert_eq!(abc[0], 0xba);
        assert_eq!(abc[1], 0x78);
        assert_eq!(abc[31], 0xad);
    }

    // ---- comparison -------------------------------------------------------------------

    #[test]
    fn a_payload_comparison_covers_every_case() {
        let bytes = patterned(100);
        let digest = digest_of(&bytes);
        assert_eq!(
            compare_payload(hashed(&bytes), 100, digest),
            PayloadComparison::Match
        );
        // Same length, different content.
        let mut changed = bytes.clone();
        changed[50] ^= 1;
        assert_eq!(
            compare_payload(hashed(&changed), 100, digest),
            PayloadComparison::Mismatch
        );
        // Same digest claimed with a different length cannot match.
        assert_eq!(
            compare_payload(hashed(&bytes), 99, digest),
            PayloadComparison::Mismatch
        );
        assert_eq!(
            compare_payload(CustomPayload::Missing, 0, digest_of(b"")),
            PayloadComparison::MissingAgreed
        );
        assert_eq!(
            compare_payload(CustomPayload::Missing, 100, digest),
            PayloadComparison::MissingCustomOnly
        );
        assert_eq!(
            compare_payload(CustomPayload::Failed, 100, digest),
            PayloadComparison::CustomFailed
        );
        // A zero-byte attachment on both sides matches (it was read, and it was empty).
        assert_eq!(
            compare_payload(hashed(b""), 0, digest_of(b"")),
            PayloadComparison::Match
        );
    }

    #[test]
    fn a_clean_message_adds_only_matches() {
        let a = patterned(70_000);
        let b = b"small".to_vec();
        let mut totals = PayloadTotals::default();
        compare_message(
            &[
                by_value_side(hashed(&a)),
                OTHER_METHOD,
                by_value_side(hashed(&b)),
            ],
            &[parser_side(&a), not_by_value(), parser_side(&b)],
            &mut totals,
        );
        assert_eq!(totals.messages, 1);
        assert_eq!(totals.payload.matched, 2);
        assert_eq!(totals.payload.mismatched, 0);
        assert_eq!(totals.bytes_matched, 70_005);
        assert_eq!(totals.largest_payload_matched, 70_000);
        assert_eq!(totals.multiset.matched, 1);
        assert_eq!(totals.gate_violations(), 0);
    }

    #[test]
    fn a_changed_byte_is_counted_as_a_mismatch_and_violates_the_gate() {
        let a = patterned(1000);
        let mut other = a.clone();
        other[10] ^= 0xFF;
        let mut totals = PayloadTotals::default();
        compare_message(
            &[by_value_side(hashed(&a))],
            &[parser_side(&other)],
            &mut totals,
        );
        assert_eq!(totals.payload.mismatched, 1);
        assert_eq!(totals.payload.matched, 0);
        assert_eq!(totals.multiset.mismatched, 1);
        assert_eq!(totals.gate_violations(), 1);
    }

    #[test]
    fn a_difference_of_order_is_a_positional_mismatch_but_a_multiset_match() {
        let a = b"first".to_vec();
        let b = b"second".to_vec();
        let mut totals = PayloadTotals::default();
        compare_message(
            &[by_value_side(hashed(&a)), by_value_side(hashed(&b))],
            &[parser_side(&b), parser_side(&a)],
            &mut totals,
        );
        assert_eq!(totals.payload.mismatched, 2);
        assert_eq!(totals.multiset.matched, 1);
        assert_eq!(totals.multiset.mismatched, 0);
        assert!(totals.gate_violations() > 0);
    }

    #[test]
    fn a_missing_stream_agrees_only_with_an_empty_payload() {
        let mut totals = PayloadTotals::default();
        compare_message(
            &[
                by_value_side(CustomPayload::Missing),
                by_value_side(CustomPayload::Missing),
            ],
            &[parser_side(b""), parser_side(b"data")],
            &mut totals,
        );
        assert_eq!(totals.missing_agreed, 1);
        assert_eq!(totals.missing_custom_only, 1);
        assert_eq!(totals.payload.matched, 0);
        assert_eq!(totals.gate_violations(), 1);
    }

    #[test]
    fn a_failed_read_violates_the_gate_and_spoils_the_multiset() {
        let mut totals = PayloadTotals::default();
        compare_message(
            &[by_value_side(CustomPayload::Failed)],
            &[parser_side(b"data")],
            &mut totals,
        );
        assert_eq!(totals.custom_failed, 1);
        assert_eq!(totals.multiset.mismatched, 1);
        assert_eq!(totals.gate_violations(), 1);
    }

    #[test]
    fn a_method_disagreement_and_a_count_difference_are_counted() {
        let mut totals = PayloadTotals::default();
        compare_message(
            &[by_value_side(hashed(b"x")), OTHER_METHOD],
            &[not_by_value(), parser_side(b"y")],
            &mut totals,
        );
        assert_eq!(totals.method_disagreement, 2);
        assert_eq!(totals.count_mismatch_messages, 0);

        let mut totals = PayloadTotals::default();
        compare_message(&[OTHER_METHOD], &[], &mut totals);
        assert_eq!(totals.count_mismatch_messages, 1);
        assert_eq!(totals.attachments_custom, 1);
        assert_eq!(totals.attachments_msg_parser, 0);
        assert_eq!(totals.gate_violations(), 1);
    }

    #[test]
    fn a_message_with_no_attachments_is_clean() {
        let mut totals = PayloadTotals::default();
        compare_message(&[], &[], &mut totals);
        assert_eq!(totals.messages, 1);
        assert_eq!(totals.multiset.matched, 1);
        assert_eq!(totals.gate_violations(), 0);
    }

    #[test]
    fn open_errors_violate_the_gate() {
        let totals = PayloadTotals {
            open_errors_msg_parser: 1,
            open_errors_custom: 2,
            ..PayloadTotals::default()
        };
        assert_eq!(totals.gate_violations(), 3);
    }

    // ---- the custom side, read through the export's own reader ------------------------

    fn entry(ty: u16, id: u16, value: [u8; 8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&value);
        v
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-vatt-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_msg(path: &Path, attachments: &[(i32, Option<&[u8]>)]) {
        let mut comp = cfb::create(path).expect("create CFB");
        put(&mut comp, "/__properties_version1.0", &[0u8; 32]);
        for (index, (method, data)) in attachments.iter().enumerate() {
            let storage = format!("/__attach_version1.0_#{index:08X}");
            comp.create_storage(&storage).expect("create storage");
            let mut props = vec![0u8; 8];
            let mut value = [0u8; 8];
            value[..4].copy_from_slice(&method.to_le_bytes());
            props.extend(entry(0x0003, 0x3705, value));
            put(
                &mut comp,
                &format!("{storage}/__properties_version1.0"),
                &props,
            );
            if let Some(data) = data {
                put(&mut comp, &format!("{storage}/__substg1.0_37010102"), data);
            }
        }
    }

    #[test]
    fn the_custom_side_hashes_what_the_export_would_write() {
        let dir = temp_dir("custom");
        let file = dir.join("m.msg");
        let payload = patterned(2 * COPY_CHUNK_BYTES + 123);
        write_msg(
            &file,
            &[
                (1, Some(payload.as_slice())),
                (6, None),
                (1, None),
                (1, Some(&b""[..])),
            ],
        );
        let OpenMessage {
            content,
            attachments: mut handle,
        } = open_message(&file).expect("open");
        let sides = custom_sides(&content, &mut handle);
        assert_eq!(sides.len(), 4);
        assert_eq!(sides[0].payload, Some(hashed(&payload)));
        assert_eq!(sides[1].payload, None, "OLE is not by value");
        assert_eq!(sides[2].payload, Some(CustomPayload::Missing));
        assert_eq!(sides[3].payload, Some(hashed(b"")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_without_a_container_is_an_open_error_not_a_panic() {
        let dir = temp_dir("broken");
        let file = dir.join("broken.msg");
        std::fs::write(&file, b"not a compound file").unwrap();
        let totals = collect_payload_totals(&[file]);
        assert_eq!(totals.open_errors_msg_parser, 1);
        assert_eq!(totals.messages, 0);
        assert!(totals.gate_violations() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the command line -------------------------------------------------------------

    #[test]
    fn the_flag_conflicts_with_the_other_modes_and_accepts_recursive() {
        assert!(Args::try_parse_from(["tsp", "dir", "--verify-attachments"]).is_ok());
        assert!(
            Args::try_parse_from(["tsp", "dir", "--verify-attachments", "--recursive"]).is_ok()
        );
        for other in [
            "--verify",
            "--verify-envelope",
            "--verify-deencap",
            "--verify-bodies",
            "--bakeoff-md",
        ] {
            assert!(
                Args::try_parse_from(["tsp", "dir", "--verify-attachments", other]).is_err(),
                "{other}"
            );
        }
        assert!(
            Args::try_parse_from(["tsp", "dir", "--verify-attachments", "--out", "o"]).is_err()
        );
        assert!(
            Args::try_parse_from([
                "tsp",
                "dir",
                "--verify-attachments",
                "--out",
                "o",
                "--dry-run"
            ])
            .is_err()
        );
    }
}
