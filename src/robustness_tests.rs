//! Robustness tests (review hardening): the parsers and converters that read untrusted bytes or
//! text must neither panic nor hand back something the export then trips over, whatever they are
//! given. Property tests over arbitrary and markup-shaped input, plus the boundary cases of the
//! bounded RTF decompression and the panic containment.
//!
//! These complement the unit tests next to each module; they are the cheap end of what a fuzzer
//! would do (a fuzz target needs the crate split into a library and a binary).

use std::io::Write as _;
use std::sync::mpsc;
use std::time::Duration;

use proptest::prelude::*;

use crate::body::{
    attachment_link, collapse_invisible_padding, content_id_key, find_cid_references,
    nests_deeper_than, preprocess_html, preprocess_html_detailed,
};
use crate::oxmsg_decode::{
    decode_properties_stream, decode_string8_with_codepage, decode_unicode_value,
};
use crate::rtf_deencap::{Limits, deencapsulate_html, recognize};
use crate::shared::{MAX_DECOMPRESSED_RTF_BYTES, decompress_rtf_bounded, run_contained};

/// Text made of the pieces that matter to the HTML and Markdown code: tag openers and closers,
/// comments, raw-text elements, Word markers, `cid:` links, invisible characters, and non-ASCII.
fn markup_soup() -> impl Strategy<Value = String> {
    let pieces: Vec<&'static str> = vec![
        "<",
        ">",
        "</",
        "/>",
        "<!--",
        "-->",
        "<style>",
        "</style>",
        "<script>",
        "<title>",
        "<![if x]>",
        "<![endif]>",
        "<o:p>",
        "</o:p>",
        "<div>",
        "</div>",
        "<p>",
        "<td>",
        "<br>",
        "](",
        "cid:",
        "<cid:",
        "![x](",
        "%",
        "%4",
        " ",
        "\n",
        "\"",
        "\u{e9}",
        "\u{200B}",
        "\u{034F}",
        "\u{1F600}",
    ];
    prop::collection::vec(prop::sample::select(pieces), 0..120).prop_map(|parts| parts.concat())
}

// ----- panic containment -----------------------------------------------------------------------

/// How long a nesting test may run before the whole test process is ended (see [`must_finish`]).
/// Generous: the tests take microseconds, but they can wait for the panic-hook mutex while other
/// tests in the same process hold it.
const NESTING_TEST_DEADLINE: Duration = Duration::from_secs(60);

/// Runs `body` and ends the test process, with a message on stderr, if it has not finished within
/// [`NESTING_TEST_DEADLINE`].
///
/// Why the process and not only the test: a nested `run_contained` that deadlocks (the defect of
/// 2026-11-09) leaves its thread holding `PANIC_HOOK_LOCK`, so every other test that calls
/// `run_contained` blocks behind it and `cargo test` hangs without output, for hours on CI. A
/// watchdog that failed only one test could not free the others. The message is written to the
/// stderr handle directly because `eprintln!` output is captured per test and would be lost.
fn must_finish<T>(name: &'static str, body: impl FnOnce() -> T) -> T {
    let (done, finished) = mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        // A panic in `body` drops `done`, which wakes this receiver with `Disconnected`, so the
        // watchdog ends quietly and the panic reaches the test harness as usual.
        let outcome = finished.recv_timeout(NESTING_TEST_DEADLINE);
        if outcome == Err(mpsc::RecvTimeoutError::Timeout) {
            let seconds = NESTING_TEST_DEADLINE.as_secs();
            let mut err = std::io::stderr();
            let _ = writeln!(err, "test `{name}` did not finish in {seconds} s");
            let _ = writeln!(err, "run_contained may not be reentrant");
            std::process::exit(1);
        }
    });
    let result = body();
    let _ = done.send(());
    let _ = watchdog.join();
    result
}

#[test]
fn a_contained_call_returns_its_value() {
    assert_eq!(run_contained(|| 41 + 1), Some(42));
}

#[test]
fn a_panic_inside_a_contained_call_becomes_none() {
    let outcome = run_contained(|| -> u32 { panic!("expected: this panic is contained") });
    assert_eq!(outcome, None);
    // And the next contained call is unaffected.
    assert_eq!(run_contained(|| "fine"), Some("fine"));
}

#[test]
fn nested_contained_calls_on_the_same_thread_succeed() {
    must_finish("same-thread nesting", same_thread_nesting_body);
}

fn same_thread_nesting_body() {
    // The PST export path nests run_contained (open_message) around decompress_rtf_bounded,
    // which itself calls run_contained. Nesting must not deadlock.
    let outer = run_contained(|| {
        let inner = run_contained(|| 20 + 2);
        assert_eq!(inner, Some(22));
        let deeper = run_contained(|| run_contained(|| "nested").unwrap());
        assert_eq!(deeper, Some("nested"));
        100
    });
    assert_eq!(outer, Some(100));
    // After the outer call finishes, a fresh top-level call still works.
    assert_eq!(run_contained(|| 7), Some(7));
}

#[test]
fn a_panic_in_an_inner_contained_call_is_contained_and_does_not_poison_the_outer() {
    must_finish("inner panic", inner_panic_body);
}

fn inner_panic_body() {
    let outer = run_contained(|| {
        let inner = run_contained(|| -> i32 { panic!("expected: inner panic") });
        assert_eq!(inner, None);
        // Outer continues and returns a value.
        99
    });
    assert_eq!(outer, Some(99));
    // Subsequent top-level call is unaffected.
    assert_eq!(run_contained(|| "recovered"), Some("recovered"));
}

#[test]
fn nested_decompress_rtf_bounded_under_outer_containment_does_not_deadlock() {
    must_finish("nested decompression", nested_decompression_body);
}

fn nested_decompression_body() {
    // Exact shape of the PST export path: outer run_contained around a call that itself
    // enters run_contained via decompress_rtf_bounded. The crafted header is long enough
    // and declares a small output so the early rejects are skipped and the inner
    // run_contained (around compressed_rtf::decompress_rtf) is actually reached.
    let outcome = run_contained(|| {
        // Minimal header: 16 bytes, small declared raw size, LZFu magic. Body may be
        // nonsense; the point is that decompress_rtf_bounded reaches its run_contained.
        let mut bytes = vec![0u8; 32];
        bytes[4..8].copy_from_slice(&16u32.to_le_bytes()); // raw_size well under the limit
        bytes[8..12].copy_from_slice(b"LZFu");
        // Result may be Some or None depending on the crate; we only care that we return.
        let _ = decompress_rtf_bounded(&bytes);

        // Also exercise a path that is refused before the inner call, for completeness.
        assert_eq!(decompress_rtf_bounded(&[]), None);

        "ok"
    });
    assert_eq!(outcome, Some("ok"));
}

// ----- the bounded RTF decompression -----------------------------------------------------------

#[test]
fn input_shorter_than_the_header_is_refused() {
    assert_eq!(decompress_rtf_bounded(&[]), None);
    assert_eq!(decompress_rtf_bounded(&[0u8; 15]), None);
}

#[test]
fn a_header_that_declares_too_large_an_output_is_refused_before_decompression() {
    let mut header = [0u8; 32];
    header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    header[8..12].copy_from_slice(b"LZFu");
    assert_eq!(decompress_rtf_bounded(&header), None);

    let mut just_over = [0u8; 32];
    let declared = u32::try_from(MAX_DECOMPRESSED_RTF_BYTES + 1).expect("the limit fits a u32");
    just_over[4..8].copy_from_slice(&declared.to_le_bytes());
    just_over[8..12].copy_from_slice(b"LZFu");
    assert_eq!(decompress_rtf_bounded(&just_over), None);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_the_decompression(
        bytes in prop::collection::vec(any::<u8>(), 0..600),
    ) {
        let _ = decompress_rtf_bounded(&bytes);
    }

    #[test]
    fn header_shaped_bytes_never_panic_the_decompression(
        compressed_size in any::<u32>(),
        raw_size in 0u32..100_000,
        mela in any::<bool>(),
        crc in any::<u32>(),
        body in prop::collection::vec(any::<u8>(), 0..600),
    ) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&compressed_size.to_le_bytes());
        bytes.extend_from_slice(&raw_size.to_le_bytes());
        bytes.extend_from_slice(if mela { b"MELA" } else { b"LZFu" });
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&body);
        let _ = decompress_rtf_bounded(&bytes);
    }
}

// ----- the MS-OXMSG decoding primitives --------------------------------------------------------

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_the_properties_stream_decoder(
        bytes in prop::collection::vec(any::<u8>(), 0..400),
        header_len in prop_oneof![Just(8usize), Just(24usize), Just(32usize), 0usize..64],
    ) {
        let _ = decode_properties_stream(&bytes, header_len);
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_string_decoders(
        bytes in prop::collection::vec(any::<u8>(), 0..200),
        code_page in prop_oneof![
            Just(1252u32), Just(28591u32), Just(20127u32), Just(65001u32), Just(1251u32),
            Just(932u32), any::<u32>(),
        ],
    ) {
        let _ = decode_string8_with_codepage(&bytes, code_page);
        let _ = decode_unicode_value(&bytes);
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_rtf_deencapsulation(
        bytes in prop::collection::vec(any::<u8>(), 0..400),
    ) {
        let _ = recognize(&bytes);
        let _ = deencapsulate_html(&bytes, Limits::default());
    }
}

// ----- the HTML and Markdown handling ----------------------------------------------------------

proptest! {
    #[test]
    fn preprocessing_never_panics_and_only_removes(
        html in prop_oneof![markup_soup(), ".{0,300}"],
    ) {
        prop_assert!(preprocess_html(&html).len() <= html.len());
        let (text, _applied) = preprocess_html_detailed(&html);
        prop_assert!(text.len() <= html.len());
    }

    #[test]
    fn collapsing_padding_never_panics_and_never_grows_the_text(
        text in prop_oneof![markup_soup(), ".{0,300}"],
    ) {
        let (clean, _runs) = collapse_invisible_padding(&text);
        prop_assert!(clean.len() <= text.len());
    }

    #[test]
    fn cid_references_always_lie_on_character_boundaries_inside_the_text(
        markdown in prop_oneof![markup_soup(), ".{0,300}"],
    ) {
        for reference in find_cid_references(&markdown) {
            prop_assert!(reference.start <= reference.end);
            prop_assert!(reference.end <= markdown.len());
            prop_assert!(markdown.is_char_boundary(reference.start));
            prop_assert!(markdown.is_char_boundary(reference.end));
        }
    }

    #[test]
    fn the_nesting_scan_never_panics_and_is_monotonic_in_the_limit(
        html in prop_oneof![markup_soup(), ".{0,300}"],
    ) {
        let deep = nests_deeper_than(&html, 50);
        let shallow = nests_deeper_than(&html, 5);
        prop_assert!(!deep || shallow);
        prop_assert!(!nests_deeper_than(&html, usize::MAX));
    }

    #[test]
    fn content_ids_and_attachment_links_never_panic_and_links_stay_inert(
        text in prop_oneof![markup_soup(), ".{0,100}"],
    ) {
        let _ = content_id_key(&text);
        let link = attachment_link(&text);
        prop_assert!(
            link.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~%/".contains(&b)),
            "a link must hold only unreserved characters, percent escapes, and the slash"
        );
        prop_assert!(link.starts_with("attachments/"));
    }
}
