---
status: "accepted"
date: 2026-10-31
decision-makers: Craig Randall (project owner)
consulted: none
informed: none
---

# Decode legacy code pages with `encoding_rs`

Status history: proposed 2026-10-30; accepted 2026-10-31. Evidence update 2026-11-11 (no change to the decision; see the end of the Context). Implemented 2026-11-15 in release `v0.1.43` (plan decision 16), with the findings and the two departures from items 4 and 7 recorded in "Implementation and results" after the Confirmation section.

## Context and Problem Statement

`teaspoon` decodes text stored in a Windows code page in two places. The `.msg` adapter decodes
`PT_STRING8` values through a chain (message code page, then Internet code page, then
Windows-1252) with in-house decoders for only four pages: 1252, 28591, 20127, and 65001. The RTF
de-encapsulation (`rtf_deencap.rs`) reuses those decoders for `\ansicpg` and font charsets; any
other page becomes U+FFFD per byte and is counted (`deencap_undecodable_bytes_total`,
`deencap_unsupported_codepage_<n>_files`). On the one-producer corpus this cost exactly one byte
(code page 1255).

M4i reads PST text, and an ANSI-format PST stores its text as `PT_STRING8` in the store's code
page (fixture priority 1 in the plan). The PST readers do not use the chain today: `source_pst.rs`
and `pst.rs` take `PropertyValue::String8(v)` through the crate's `to_string()`. The plan's
earlier position was to add a code page crate only if a second producer's fixtures showed more
than the one byte seen. On 2026-10-30 the owner decided to adopt `encoding_rs` as M4i groundwork.
The need is therefore anticipated (ANSI PSTs), not yet demonstrated by a fixture: the demonstrated
cost of not adopting is one byte. This ADR records how it is adopted.

Update, 2026-11-11: the cost is now demonstrated on a second fixture, for RTF text and not for `PT_STRING8`. In the export of `crkb.pst`, 29 of 6,508 messages (0.45%) carry RTF text in another code page: 26 of their RTF files declare Windows-1256 (Arabic), so the de-encapsulation replaced 562 non-ASCII bytes with U+FFFD and counted them (`undecodable_bytes_replaced`). The received RTF is kept as `body.rtf`, so the archive loses nothing, but `message.md` and the recovered `body.html` are wrong where those bytes were non-ASCII. Three of the 29 declare 1252 and are not explained. `crkb.pst` holds no `PT_STRING8` value, so the need for ANSI PSTs is still anticipated. The decision stands; the plan treats the timing as an open decision (plan decision 16).

## Decision Drivers

- Loss is explicit: a byte that cannot be decoded is counted, never dropped silently.
- Deterministic output: the decoded text is archive content.
- No panic on a hostile or corrupt file.
- Licence compatible with `MIT OR Apache-2.0`.
- Fidelity to what Outlook wrote, which is Windows code page semantics, not web semantics, and
  no change to decoding that is already verified (M3 parity with `msg_parser`).
- One definition per fact: one table from a code page number to a decoder.
- Cheap to reverse: the choice should be confined to one module.

## Considered Options

1. Keep the in-house decoders and add pages as fixtures show a need.
2. `encoding_rs` for every code page, with its (WHATWG Encoding Standard) semantics.
3. `encoding_rs` for the pages where its decoding equals the Windows page, in-house decoders
   where it does not (28591, 20127), and a counted loss for pages it does not cover.
4. A crate that takes Windows code page numbers directly (`codepage-strings`, which an early
   `outlook-pst` example (0.1.0) used; not rechecked for 1.2.0). Not evaluated; recorded as the
   candidate for pages `encoding_rs` lacks (DOS/OEM pages).

## Decision Outcome

Chosen option: 3, because it is the only option that adopts `encoding_rs`, as the owner decided,
without changing the verified decoding of the two pages whose Windows meaning differs from the
web's, and because the crate sits behind one module, so the choice is cheap to reverse: if the
web's reading of 28591 proves better for real mail, or option 4 proves better for the OEM pages,
the change is confined to `src/codepage.rs`. Items 1 to 9 are accepted as recommended, including
the recommendation in item 6 for 28599 and similar pages.

1. **Dependency.** `encoding_rs = "=0.8.35"`, default features only (no `simd-accel`, no `serde`).
   Pinned with `=` because a new version can change decoded text, which is archive content.
2. **One module.** A new `src/codepage.rs` holds the table from Microsoft code page number to
   `&'static Encoding` and is the only place that names an `encoding_rs` type. `oxmsg_decode` and
   `rtf_deencap` call it. The exact list is written from Microsoft's code page identifier list
   when the module is written, and each entry gets a test.
3. **No BOM handling.** The declared code page decides. `Encoding::decode` sniffs a byte order
   mark and can override it, so it is not used; the decoder is created without BOM handling.
4. **Malformed bytes stay counted as bytes.** Decode with `Decoder::decode_to_string_without_
   replacement`, which reports each malformed sequence with its length; `tsp` writes the U+FFFD
   itself and adds the length to the existing undecodable-bytes counter, so the counter keeps its
   meaning. That method treats the `String`'s capacity as its output limit, so the caller reserves
   with `max_utf8_buffer_length_without_replacement` and loops on `OutputFull`. (The API names
   were checked against the crate's documentation on 2026-10-31; nothing here has been compiled.)
5. **Unmapped pages stay a counted loss**, under the existing `unsupported_codepage_<n>` keys.
   As far as is known, `encoding_rs` has no DOS/OEM pages (437, 850), EBCDIC, UTF-7 (65000), or
   Johab (1361); the table test asks `Encoding::for_label` for each label so this is checked, not
   assumed.
6. **Windows versus WHATWG.** The Encoding Standard decodes the labels for ISO-8859-1, US-ASCII,
   and ISO-8859-9 as Windows-1252 and Windows-1254, which differ from the ISO pages at bytes
   0x80 to 0x9F (printable punctuation instead of C1 controls). Therefore 28591 and 20127 keep
   their in-house decoders (no change in behavior). For 28599 and any similar page the decision
   is to accept the WHATWG decoding and record the deviation, because only 0x80 to 0x9F differ.
   The multibyte pages (932, 936, 949, 950) are decoded with the standard's variants, which are
   expected to differ from the Windows pages in some mappings; that is believed, not verified, and
   is a recorded limit until a Windows decode of a real fixture can be compared.
7. **Retiring in-house decoders.** The 1252 and 65001 decoders are replaced only after an
   equivalence test passes: all 256 bytes for 1252, and the corpus `--verify` and
   `verify-split.ps1` outputs byte-identical for both.
8. **Containment.** Decoding is called through `shared::run_contained` once per message, not once
   per string: each call swaps the global panic hook under a lock, which is too costly per value
   on a very large PST. A property test (arbitrary bytes, every mapped page) asserts no panic and
   valid UTF-8 out.
9. **PST text (M4i).** PST strings are read from the raw buffer (`String8`'s `buffer()`) and
   decoded through the same chain, not through `to_string()`. What `String8::to_string()` does
   has not been read, so whether today's PST names are already wrong for an ANSI store is not
   known.

### Consequences

* Good, because it meets the owner's decision to adopt `encoding_rs` while the verified decoding
  of 28591 and 20127 does not change at all.
* Good, because an ANSI PST's text (fixture priority 1) becomes decodable for every page in the
  table, including the multibyte pages that cannot reasonably be written in-house.
* Good, because the one known counted loss (one byte of code page 1255 in the corpus) should
  decode (to U+200F); this is checked under Confirmation, not assumed.
* Good, because loss stays explicit: the decoder reports each malformed sequence with its length,
  so the existing counters keep their meaning.
* Good, because the crate is behind one module, so replacing it, or adding option 4 for the pages
  `encoding_rs` lacks, changes that module and nothing else.
* Good, because with default features the crate adds one small dependency (`cfg-if`).
* Neutral, because a message that used a previously unsupported page changes its decoded text, so
  the corpus tree hash is expected to change for exactly that message; that is the fix taking
  effect, and Confirmation item 3 checks that nothing else changes.
* Neutral, because binary size and build time grow by an amount not yet measured; Confirmation
  item 4 measures it and the owner rules on the result.
* Bad, because the licence is `(Apache-2.0 OR MIT) AND BSD-3-Clause`: the BSD-3-Clause term
  covers data derived from the WHATWG tables, and its notice must travel with a distributed
  `tsp.exe`, as the notices of the existing MIT and Apache-2.0 dependencies already must. The
  repository is `publish = false`, but the obligation applies if the binary is shared.
* Bad, because a new dependency is new supply-chain surface, and its source has not been read for
  panics on hostile input; it is pinned with `=`, called through `run_contained`, and covered by
  the property test of item 8 before it is relied on.
* Bad, because two in-house decoders (28591 and 20127) remain beside the crate.
* Bad, because for the multibyte pages the standard's decoders are expected to differ from the
  Windows pages in some mappings (item 6); a recorded limit, not measured.

### Confirmation

1. `cargo tree --depth 1` and `cargo tree -p encoding_rs` before and after; the licence line
   re-read from the pinned version.
2. The 1252 equivalence test and the UTF-8 comparison of item 7; `verify-split.ps1` default,
   `--verify`, and PST outputs IDENTICAL.
3. On the corpus, `deencap_undecodable_bytes_total` 1 to 0 and the 1255 file count 1 to 0, with
   a corpus export that differs in that message's files only (per-file hashes compared).
4. The property test of item 8, and release build times with and without the dependency.
5. An ANSI PST fixture read by the owner, since no oracle exists for PST text. This is the check
   that shows whether the anticipated need was real.
6. On `crkb.pst` (added 2026-11-11), the 29 messages: `export_body_loss_undecodable_bytes_replaced` falls from 562 to the bytes of code pages that are still unmapped, the 26 Windows-1256 files decode, and the export differs from the previous one in those messages' files and in the version string of the root `folder.json` only (per-file hashes compared); the 3 files that declare 1252 are looked at separately.

## Implementation and results (added 2026-11-15, release `v0.1.43`)

Items 1 to 9 were built as accepted, in the code commit `245fdc8` (11 files; CI green; 459 tests pass, 26 of them new). What the build changed or found:

1. **The table has 31 pages** (`src/codepage.rs`, the only module that names an `encoding_rs` type): 866, 874, 932, 936, 949, 950, the Windows pages 1250, 1251, and 1253 to 1258, 10000 (Mac Roman), 20866 (KOI8-R), 21866 (KOI8-U), 28592 to 28599 (28599 as windows-1254, the recorded deviation of item 6; a test pins 0x80 to the euro sign), 28603, 28605, 38598, 51932, 51949, and 54936. Each has a test with a known byte sequence, and a test keeps the list and the table in step. Pages known to be absent from the standard (437, 850, 852, EBCDIC, Johab, UTF-7) are checked through `Encoding::for_label`, as item 5 asked. Not in the table: 1252, 28591, 20127, and 65001, which stay in-house.
2. **Item 7 is amended: the Windows-1252 decoder is not retired.** The Encoding Standard's windows-1252 maps the five bytes Microsoft leaves undefined (0x81, 0x8D, 0x8F, 0x90, 0x9D) to C1 controls (recalled, not re-checked against the published index), where `tsp` replaces and counts them, so the 256-byte equivalence test that item 7 requires first cannot pass, and the in-house decoder stays. The UTF-8 decoder also stays: it is now exact (item 3 below).
3. **Item 4 is amended: the UTF-8 count is exact.** Both UTF-8 paths (native HTML, and 65001 in the `PT_STRING8` chain) now use `Utf8Chunks` (`oxmsg_decode::decode_utf8_counted`): one U+FFFD for each maximal invalid sequence, as `String::from_utf8_lossy` writes it, and the sequence's length added to the count. The old count was the number of U+FFFD characters in the result, which also counted any that were valid in the source. In the RTF path the count of replaced bytes under a page that has a decoder was dropped (only the pages with no decoder were counted); it is added now, so the five undefined Windows-1252 bytes can no longer become U+FFFD without a record. Neither case occurred on `crkb.pst`.
4. **Item 8: containment is once per message**, in `archive::body_outcome_with` around `select_body`. A contained panic falls back to the plain-text body, if any, and is recorded as `html_conversion_failed` (no new loss reason). The property tests decode arbitrary bytes under every mapped page (`codepage`), and through `decode_string8_with_codepage` for any page number (`robustness_tests`). The contained path has been exercised by a unit test of the fallback only; nothing makes the crate panic.
5. **A finding in the RTF code:** the interpreter wrote ASCII-range bytes straight to the output, but a trail byte of Shift_JIS, GBK, or Big5 can be below 0x80 (Big5 U+4E00 is `A4 40`, and the `40` arrives as a `\'40` escape), which would have split the character. While a run of a multibyte page is open, every byte now joins the run and the decoder decides. Tested on synthetic RTF only; no real message uses a multibyte page.
6. **The decode record** that plan decision 17 asked for is the `body.decoding` object (body ADR, decision 10).

**Results (owner's runs, 2026-11-14 and 2026-11-15; details in `docs/verification/m4-results.md`).** Confirmation item 6: on `crkb.pst`, `export_body_loss_undecodable_bytes_replaced` fell from 562 to 0, and 26 messages record page 1256 as in effect with no page recorded as replaced (the page counts for the whole export are 65001 in 4,538 messages, 1252 in 130, 1256 in 26, 28591 in 3, 1255 in 2, and 1250 in 1). The two exports, made by the `v0.1.42` and `v0.1.43` builds at output roots of the same length and compared file by file (a check script outside the crate), differ in the `body.html` and `message.md` of the 29 messages and in their `metadata.json` (`loss` and `originals`), in the added `body.decoding` of every message with an HTML body, and in the tool version of the root `folder.json`; no other file differs (attachments compared by size in that run; an earlier full-hash run found none changed at matching paths). The 3 messages that declare 1252 no longer lose bytes; the page counts for 1255 (2 messages) and 1250 (1) fit them, but those counts are for the whole export, so which pages those 3 used is not shown. Confirmation item 3: the corpus export reports 0 undecodable bytes (it was 1, in code page 1255); its tree hash changed, as it must, because every HTML-bodied message gains `body.decoding`, and it was not compared file by file. Confirmation item 4: the property tests pass; build times were not measured. Confirmation items 1 (the `cargo tree` and licence lines) and 5 (an ANSI PST fixture) were not run, and item 2's 1252 equivalence is replaced by the departure above.

**What the build does not show.** No multibyte page, and no `PT_STRING8` value, has met real data; the multibyte decoders are the standard's, which may differ from Microsoft's in some mappings (not measured). No one has yet read a decoded Arabic message by eye. The BSD-3-Clause notice obligation (see Consequences) has not been acted on. The API names of item 4 now compile and the tests pass, which closes the "not verified: that the API names compile" line of More Information; the panic-freedom of the crate on hostile input is still supported only by the property tests.

## Pros and Cons of the Options

### 1. Keep the in-house decoders

* Good, because nothing new is depended on, and the demonstrated cost of staying is one byte on a
  one-producer corpus.
* Bad, because every page costs a table and a test, the multibyte pages cannot reasonably be
  written in-house, and ANSI PSTs (fixture priority 1) are the likely place the cost appears.

### 2. `encoding_rs` for every page

* Good, because it is the simplest.
* Bad, because it changes the decoding of bytes 0x80 to 0x9F for 28591 and 20127 relative to
  Windows and to the verified M3 behavior, and no counter would show the change. For mislabeled
  mail the web's reading might well read better; that is a reason to revisit item 6 with real
  data, not a reason to change verified behavior silently.

### 3. `encoding_rs` plus in-house exceptions (chosen)

* Good, because behavior of the existing pages is unchanged and the crate is isolated in one
  module.
* Bad, because two decoders remain beside the crate.

### 4. A crate keyed by Windows code page numbers

* Good, because it may cover the OEM pages `encoding_rs` lacks.
* Neutral, because it is not evaluated (licence, dependencies, hostile-input behavior unread).

## More Information

Verified from the crate's published metadata on 2026-10-30: version 0.8.35; licence
`(Apache-2.0 OR MIT) AND BSD-3-Clause`; normal dependency `cfg-if` only with default features
(`any_all_workaround` only with `simd-accel`, `serde` optional). Verified on 2026-10-31 from
published sources: the Encoding Standard labels `iso-8859-1`, `latin1`, and `us-ascii` decode as
windows-1252 and `iso-8859-9` as windows-1254, and those pages differ from the ISO pages only at
0x80 to 0x9F; the `Decoder::decode_to_string_without_replacement` signature and its
`Malformed`, `OutputFull`, and `InputEmpty` results.

Not verified: that 0.8.35 is the latest release; that the crate has no panic path on hostile
input (its source is not read, and wide deployment is not proof); that `cfg-if` is already in
`Cargo.lock`; binary size and build time; that the API names compile in this project; the
multibyte differences of item 6; that `codepage-strings` is still what `outlook-pst` uses.

Related: [Microsoft specifications as normative authority](microsoft-specifications-as-normative-authority.md)
(code page numbers come from Microsoft's list, not from the web's labels), and the
[M4a dependency research](../docs/plans/m4a-dependency-research.md) (every new dependency: pin,
read the source, record the licence, run `cargo tree`).
