---
status: "proposed"
date: 2026-10-30
decision-makers: Craig Randall (project owner)
consulted: none
informed: none
---

# Decode legacy code pages with `encoding_rs`

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
This ADR records how it is adopted.

## Decision Drivers

- Loss is explicit: a byte that cannot be decoded is counted, never dropped silently.
- Deterministic output: the decoded text is archive content.
- No panic on a hostile or corrupt file.
- Licence compatible with `MIT OR Apache-2.0`.
- Fidelity to what Outlook wrote, which is Windows code page semantics, not web semantics.
- One definition per fact: one table from a code page number to a decoder.

## Considered Options

1. Keep the in-house decoders and add pages as fixtures show a need.
2. `encoding_rs` for every code page, with its (WHATWG Encoding Standard) semantics.
3. `encoding_rs` for the pages where its decoding equals the Windows page, in-house decoders
   where it does not (28591, 20127), and a counted loss for pages it does not cover.
4. A crate that takes Windows code page numbers directly (`codepage-strings`, which the
   `outlook-pst` examples use). Not evaluated; recorded as the candidate for pages `encoding_rs`
   lacks (DOS/OEM pages).

## Decision Outcome

Chosen option: 3, because it adopts `encoding_rs` as the owner decided without changing the
decoding of the two pages whose Windows meaning differs from the web's. Each item below is
proposed as recommended and is open to the owner's ruling.

1. **Dependency.** `encoding_rs = "=0.8.35"`, default features only (no `simd-accel`, no `serde`).
   Pinned with `=` because a new version can change decoded text, which is archive content.
2. **One module.** A new `src/codepage.rs` holds the table from Microsoft code page number to
   `&'static Encoding` and is the only place that names an `encoding_rs` type. `oxmsg_decode` and
   `rtf_deencap` call it. The exact list is written from Microsoft's code page identifier list
   when the module is written, and each entry gets a test.
3. **No BOM handling.** The declared code page decides. `Encoding::decode` sniffs a byte order
   mark and can override it, so it is not used.
4. **Malformed bytes stay counted as bytes.** Decode with `Decoder::decode_to_string_without_
   replacement`, which reports each malformed sequence with its length; `tsp` writes the U+FFFD
   itself and adds the length to the existing undecodable-bytes counter, so the counter keeps its
   meaning. (The API names are from the crate's documentation and have not been compiled.)
5. **Unmapped pages stay a counted loss**, under the existing `unsupported_codepage_<n>` keys.
   As far as is known, `encoding_rs` has no DOS/OEM pages (437, 850), EBCDIC, UTF-7 (65000), or
   Johab (1361); the table test asks `Encoding::for_label` for each label so this is checked, not
   assumed.
6. **Windows versus WHATWG.** The Encoding Standard decodes the labels for ISO-8859-1, US-ASCII,
   and ISO-8859-9 as Windows-1252 and Windows-1254, which differ from the ISO pages at bytes
   0x80 to 0x9F (printable punctuation instead of C1 controls). Therefore 28591 and 20127 keep
   their in-house decoders (no change in behavior). For 28599 and any similar page the owner
   decides between accepting the WHATWG decoding and recording the deviation (recommended,
   because only 0x80 to 0x9F differ) or leaving the page unmapped and counted. The multibyte
   pages (932, 936, 949, 950) are decoded with the standard's variants, which are supersets of
   the Windows pages but not identical in every mapping; this is a recorded limit.
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

* Good, because the Windows-1255 byte should decode (U+200F) and an ANSI PST's text becomes
  decodable at all; both are checked under Confirmation, not assumed.
* Good, because one table replaces per-page decoders, and the unsupported-page counters keep
  their meaning.
* Bad, because the licence is `(Apache-2.0 OR MIT) AND BSD-3-Clause`: the BSD-3-Clause term
  covers data derived from the WHATWG tables, and its notice must travel with a distributed
  `tsp.exe`. The repository is `publish = false`, but the obligation applies if the binary is
  shared.
* Bad, because the standard's decoding is not byte-for-byte Windows decoding for the multibyte
  pages and the aliased ISO pages (item 6).
* Bad, because binary size and build time grow by an amount not yet measured.
* Bad, because a message that used an unsupported-then-supported page changes its decoded text, so
  the corpus tree hash can change for exactly that message.

### Confirmation

1. `cargo tree --depth 1` and `cargo tree -p encoding_rs` before and after; the licence line
   re-read from the pinned version.
2. The 1252 equivalence test and the UTF-8 comparison of item 7; `verify-split.ps1` default,
   `--verify`, and PST outputs IDENTICAL.
3. On the corpus, `deencap_undecodable_bytes_total` 1 to 0 and the 1255 file count 1 to 0, with
   a corpus export that differs in that message's files only (per-file hashes compared).
4. The property test of item 8, and release build times with and without the dependency.
5. An ANSI PST fixture read by the owner, since no oracle exists for PST text.

## Pros and Cons of the Options

### 1. Keep the in-house decoders

* Good, because nothing new is depended on.
* Bad, because every page costs a table and a test, and the multibyte pages cannot reasonably be
  written in-house.

### 2. `encoding_rs` for every page

* Good, because it is the simplest.
* Bad, because it silently changes the meaning of 28591 and 20127 bytes 0x80 to 0x9F, which is
  loss that nothing counts.

### 3. `encoding_rs` plus in-house exceptions (chosen)

* Good, because behavior of the existing pages is unchanged.
* Bad, because two decoders remain beside the crate.

### 4. A crate keyed by Windows code page numbers

* Good, because it may cover the OEM pages `encoding_rs` lacks.
* Neutral, because it is not evaluated (licence, dependencies, hostile-input behavior unread).

## More Information

Verified on 2026-10-30 from the crate's published metadata: version 0.8.35; licence
`(Apache-2.0 OR MIT) AND BSD-3-Clause`; normal dependency `cfg-if` only with default features
(`any_all_workaround` only with `simd-accel`, `serde` optional). Not verified: that 0.8.35 is the
latest release; that the crate has no panic path on hostile input (its source is not read, and
wide deployment is not proof); that `cfg-if` is already in `Cargo.lock`; binary size and build
time; that the API names above compile.

Related: [Microsoft specifications as normative authority](microsoft-specifications-as-normative-authority.md)
(code page numbers come from Microsoft's list, not from the web's labels), and the
[M4a dependency research](../docs/plans/m4a-dependency-research.md) (every new dependency: pin,
read the source, record the licence, run `cargo tree`).
