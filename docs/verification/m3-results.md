# M3 — MSG value extraction and default-path graduation

## Status

**M3a–M3f: complete and verified against the full 29-file corpus.** The default `.msg` path is the custom MS-OXMSG extraction path. The M3f flip was verified by running the flipped default over the 29 fixtures: its output is identical to the earlier `--extract` output (`extract4.txt`), the build was clean, and all 56 tests passed. **M3g** (remove the transitional `--oxmsg` and `--extract` flags) remains.

**Post-M3f follow-ups: drafted, awaiting a Windows run.** Three changes landed after the flip was verified and have not yet been compiled or run by the project owner: the structural gates folded into `--verify`, the `PT_STRING8` code page chain (closing the M3b design debt), and the corpus-gated regression test. They are described in "Post-M3f follow-ups" below, with exactly what is and is not yet evidenced. Nothing in this document reports a result from them; the results recorded here are those of the verified M3f build.

ADR "Custom MS-OXMSG parser graduates to the production MSG path" moved from Proposed to Accepted with M3f. ADR "Independent differential verification" now records the MSG-side verification actually performed: the `--verify` harness and the `--extract` vs. default textual diff are exactly the "independent implementation as comparison oracle" work that ADR called for. The PST-side comparison is still outstanding.

## Corpus and run inputs

All results below come from the 29-file `.msg` fixture corpus (3 subdirectories deliberately skipped, non-recursive scan), run on Windows. The fixtures are not in the repository.

The M3e commands, run while `msg_parser` was still the default:

```text
cargo run --release -- "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs\" > default.txt
cargo run --release -- --extract "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs\" > extract.txt
cargo run --release -- --verify  "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs\"
cargo run --release -- --oxmsg   "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs\"
```

The M3f verification, run on the flipped build. This is the current form of the first command above, since the default now is the custom path:

```text
cargo run --release -- "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs\" > default_was_extract4.txt
```

`default_was_extract4.txt` is identical to `extract4.txt`, the `--extract` output captured before the flip.

Quality gate (unchanged from previous slices): `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo build --release`. CI (`.github/workflows/ci.yml`) runs the same on Linux and Windows, but cannot run anything that needs the fixtures.

## M3a — fixed-value interpretation

`decode_fixed_value` decodes every fixed-length entry's 8-byte value field into its real Rust type (`DecodedFixedValue`): `PT_SHORT`, `PT_LONG`, `PT_FLOAT`, `PT_DOUBLE`, `PT_CURRENCY`, `PT_APPTIME`, `PT_ERROR`, `PT_BOOLEAN`, `PT_I8`, `PT_SYSTIME`. Kept as a lossless, in-memory intermediate representation only — nothing from it is printed by the `--oxmsg` diagnostic, which stays exactly as content-free as every earlier slice. The boolean-encoding validity check from the earlier slice became real decoding here, as the M3 plan required.

Wired in as two structural checks: the existing boolean-encoding validity check, and a NaN/infinite check for `PT_FLOAT`/`PT_DOUBLE`/`PT_APPTIME`. Both read 0 on the full corpus:

```text
fixed_boolean_invalid_encoding_total=0
fixed_float_non_finite_total=0
```

Decoding is exercised for real in the extraction layer: recipient types and attach methods are read through `decode_fixed_value` (type-gated against `PT_ERROR` placeholders), producing the counts verified under M3d/M3e below.

A `PT_SYSTIME` plausibility check (flagging implausible calendar dates) was considered and deliberately not added — MS-OXOCAL's legitimate "no end date" convention for recurring calendar items lands around the year 4500, so a naive range check would misfire on real, correct data.

## M3b — variable-length value reading

PT\_UNICODE decodes as UTF-16LE, with PT\_BINARY read as raw bytes and PT\_CLSID validated at exactly 16 bytes. PT\_STRING8 was originally hand-implemented as Windows-1252 (no new dependency); it is now code page aware, see "PT_STRING8 code page chain" below. Structural validation covers the declared Size field vs. actual stream length and UTF-16 byte parity. Confirmed clean on the corpus:

```text
variable_value_stream_found_total=2017
variable_value_stream_missing_total=0
variable_value_size_mismatch_total=0
variable_value_odd_utf16_length_total=0
variable_unicode_decode_errors_total=0
variable_string8_undefined_byte_total=0
variable_clsid_wrong_length_total=0
```

(2017, not 2019, is correct: it excludes the 2 `PT_OBJECT` entries accounted for via the embedded-object-storage path.)

**Known limitation at the time of these runs, explicit per design principle 5:** the corpus contains no PT\_STRING8 property at all (every text property uses PT\_UNICODE), so the Windows-1252 decoder was implemented but unverified against real data, and `PidTagMessageCodepage` was not consulted. Because the corpus exercises no STRING8 value, this could not affect any gate-relevant field in the M3f diff. The follow-up below addresses the second half of that limitation; the first half cannot be addressed without real ANSI-encoded input.

## M3c — named-property values, including string names

The Entry/GUID/String stream decoding (M2.x groundwork, retained below) now resolves every named property. The bit-layout investigation and its fix are part of this slice's evidence: named properties resolve to 436 occurrences across the corpus with both cross-check gates at 0, and string-kind names decode successfully (`named_properties_string_decode_errors_total=0`).

```text
named_properties_seen_total=436
named_properties_guid_out_of_range_total=0
named_properties_index_mismatch_total=0
named_properties_string_kind_total=315
named_properties_numeric_kind_total=121
named_property_set set=PSETID_Common count=183
named_property_set set=PSETID_Task count=3
named_property_set set=PS_INTERNET_HEADERS count=107
named_property_set set=custom count=143
```

Per the M3 plan, this stage is legitimately about extracting real content, and string names are decoded in memory. Consistent with the tool's privacy rules, names are still never printed — the diagnostic surfaces only kind (string vs. numeric), property-set membership, and decode success/failure.

## M3d — diagnostic surface rebuilt from decoded values

`--extract` ran the custom extraction path alone (no `msg_parser`) and printed exactly the same report shape as the `msg_parser`-based default `.msg` diagnostic, populated via the same shared `BodyCounters`/`CountStats`/`ZeroByteStats` types and `record_*` functions — derived from the same normalized decoded representation M3a–M3c produce, not reimplemented ad hoc, per design principle 2. One custom-path-only anomaly key is added: `attachments_data_stream_missing` (a by-value attachment whose `PidTagAttachDataBinary` could not be read is an anomaly, not evidence of an empty file; an empty-but-readable stream is the genuine zero-byte signal).

Corpus output of the custom path (`extract4.txt`, from the build in which embedded-message opening reads each attachment's own attach method; identical to `default_was_extract4.txt` from the flipped default):

```text
inventory=privacy_safe
input_kind=msg
files_scanned=29
subdirectories_skipped=3
open_errors=0
message_class_missing=0
message_class class=IPM.Note count=29
bodies_plain=29
bodies_html=27
bodies_html_native=0
bodies_html_via_rtf=27
bodies_rtf=29
rtf_decompression_errors=0
rtf_decompressed_bytes_total=1787065
messages_with_recipients=29
recipients_to=29
recipients_cc=6
recipients_bcc=1
max_recipients_on_a_message=6
messages_with_attachments=11
total_attachments=29
max_attachments_on_a_message=11
attachments_zero_byte=1
attachments_zero_size_other_method=2
attachments_with_content_id=23
attachments_method_by_value=27
attachments_method_embedded_message=1
attachments_method_ole=1
attachments_method_other=0
embedded_messages_opened=1
embedded_message_open_errors=0
embedded_message_class class=IPM.Note count=1
attachments_data_stream_missing=0
```

### Embedded-message opening: how the count is produced

`embedded_messages_opened=1` is a real open, not a re-read of the outer message. The extraction layer gates on each attachment's own `PidTagAttachMethod` being embedded-message (5), requires a message-shaped `PidTagAttachDataObject` storage (one that directly contains `__properties_version1.0`), and reads the nested message's class from `PidTagMessageClass` inside that storage. An earlier draft could read the outer message's class and overcount the success; the `embedded_message_class_unreadable_total` counter and the one-per-attachment loop exist so that cannot recur unnoticed.

## M3e — differential verification

Two independent comparisons were run over the same corpus: (1) `--verify`, which runs both extraction paths field by field and prints match/mismatch counts only; (2) a direct textual diff of `--extract` output against the `msg_parser` default report, which is what `--extract`'s report shape was designed to support.

### Textual diff: `--extract` vs. `msg_parser` default

Out of ~27 output keys, **every field matches exactly except two value differences and one deliberate extra key**:

| Field | default (msg_parser) | extract (custom) | Triage |
|---|---|---|---|
| `rtf_decompressed_bytes_total` | 1787066 | 1787065 | **Custom path correct.** `msg_parser`'s preset LZFu dictionary diverges from MS-OXRTFCP's published 207-byte dictionary (verified directly against Microsoft's spec page, diverging around byte 195); `compressed-rtf` matches it exactly. `--verify` localized the disagreement to exactly one file of 29, with a signed delta of −1 byte, consistent with a narrow dictionary-tail divergence. Per ADR "Independent differential verification" and ADR "Microsoft specifications as normative authority", this was checked against the spec — and the custom path is the one verified correct. |
| `embedded_messages_opened` | 0 | 1 | **Documented improvement.** `msg_parser`'s `Attachment::as_message()` returns `None` (not an error) on this corpus's one embedded-message attachment — M2c's finding, root cause never identified. The custom path opens it and reads its `PidTagMessageClass` successfully (`embedded_message_class_readable_total=1`, `embedded_message_class_unreadable_total=0`), reporting `IPM.Note`. This is the "documented, justified improvement" category the M3f gate explicitly accepts. |
| `attachments_data_stream_missing` | — (absent) | 0 | Custom-path-only key, by design — the documented one-key delta of the `--extract` report shape. Value 0 on this corpus. |

The `embedded_message_class` line follows the `embedded_messages_opened` difference: it is present in the custom path's report (one `IPM.Note`) and absent from the `msg_parser` report, which opened none.

All other fields match exactly: message class (29 × `IPM.Note`), `message_class_missing`, all four body-availability counters, `rtf_decompression_errors`, all recipient counts and max, all attachment presence/total/max, `attachments_zero_byte`, `attachments_zero_size_other_method`, `attachments_with_content_id`, every attachment-method bucket, and `embedded_message_open_errors`.

Notably, `attachments_zero_byte=1` matches on both sides — the earlier zero-byte-attachment detection gap between the paths is closed: zero bytes are counted only for confirmed-empty `PidTagAttachDataBinary` streams, with unreadable streams reported separately via `attachments_data_stream_missing` (0 on this corpus).

### `--verify` field-by-field comparison

`--verify` compares message class (four outcomes: both-present match/mismatch, presence mismatch, both absent), the four body-availability booleans, recipients by type, attachment totals and method classification, content-ID presence, RTF decompressed byte lengths (with per-file signed mismatch deltas), and embedded-message opens. Results: clean on every comparable field, with the same two triaged differences above appearing as the RTF single-file −1-byte delta and the embedded-message improvement. The corpus's full `--verify` output is reproducible from the commands above.

### Recipient and attachment scoping fix found by this verification

The structural `--oxmsg` counters (`recipient_storages_total`, `attachment_storages_total`) read 37/30 against `msg_parser`'s 36/29. `--verify`'s per-recipient classification resolved this: the extra storage is the embedded message's own recipient storage, swept in by an unscoped full-tree CFB walk — not an ORIG-type recipient (`recipient_orig_total=0` ruled that out directly). The extraction layer counts only storages whose parent is the CFB root, matching `msg_parser`'s outer-message-only scope. The structural counters intentionally keep their whole-tree scope: they were built to prove every entry in the file is classified, which they do.

## M3f — default flipped

The gate condition from the M3 plan was: parity or documented, justified improvement, established by M3e's differential run. The evidence satisfied it:

- **Parity** on every comparable output field.
- **Two documented improvements**, both spec-checked rather than assumed: the RTF dictionary fix (custom path verified against Microsoft's published MS-OXRTFCP dictionary) and embedded-message opening (verified against the real fixture where `msg_parser` fails).
- Zero unexplained anomalies: `entry_accounting_gap_total=0`, no unrecognized entries, no unresolved recipients or attachments, no missing value streams.

What changed in M3f:

- `main()`'s dispatch now sends `.msg` input to the custom extraction path by default. `--verify` still takes precedence. `--extract` is a redundant alias of the default and takes precedence over `--oxmsg`; `--oxmsg` still runs the structural diagnostic when given alone.
- The `msg_parser`-based default report path and its helpers were removed. Leaving it unreachable would fail `clippy -D warnings`, and keeping it reachable would have been the runtime fallback the ADR rejected. `msg_parser` is now used only by `--verify`, as the oracle.
- One test that exercised a removed helper was removed with it: 57 tests became 56.

`msg_parser` is a normal Cargo dependency, not a dev-dependency, because `--verify` ships in the `tsp` binary. Its oracle-only role is enforced by where it is called, not by the Cargo section it appears in (see the ADR's Confirmation section).

**Verification of the flip:** the default run over the 29 fixtures produced `default_was_extract4.txt`, identical to `extract4.txt`; `cargo` build clean; 56 tests passed.

## Post-M3f follow-ups (drafted; awaiting a Windows run)

### Structural gates folded into `--verify`

The structural counters that previously ran only under `--oxmsg` now also run inside `--verify`. `--verify` calls the same structural walk (`inspect_oxmsg`) on each file it already opens, accumulates the result next to the comparison tallies, and prints:

- one line per structural gate, using the same key names `--oxmsg` used, each of which must read 0 on a clean corpus: `entry_accounting_gap_total`, `unrecognized_entries_total`, the four `properties_stream_*` anomaly counters, `attach_data_object_size_sentinel_mismatches`, the two `attach_data_object_reserved_vs_*_object_storage_gap` cross-checks, the fixed-value checks, the variable-value checks (`variable_value_stream_missing_total` and its siblings, including `variable_string8_unsupported_codepage_total`), and the three named-property checks;
- `structural_gate_violations`, the number of gates that are nonzero (0 on a clean corpus);
- a few informational, ungated counters (`total_entries`, `opaque_payload_entries_total`, the named-property "missing/unresolvable" totals, and the `PT_STRING8` code page resolution counts).

Most gates above have corpus evidence of reading 0 in the sections of this document that record them. Two need a caveat. `unrecognized_entries_total` was first reported as 62 and then fully accounted for (see "Resolution of the 62 unrecognized entries" below); the M3f evidence records no unrecognized entries, but no post-resolution `--oxmsg` output is attached to this document, so the resolved value of 0 is asserted rather than shown. `variable_string8_unsupported_codepage_total` is new with the code page chain, and is 0 by construction on a corpus with no `PT_STRING8` value. So the first `--verify` run on the corpus is what evidences the gate block as a whole: expect `structural_gate_violations=0`, and treat a nonzero `unrecognized_entries_total` as either a stale expectation in this document or a real finding, to be triaged against the resolution table.

Structural counts are signed where they are differences, so an overcount is visible. The scan loop and the printing were separated (`collect_msg_verify_totals`, `print_msg_verify_report`) so that assertions can run on the totals rather than on printed text.

This is what allows `--oxmsg` to be retired: its accounting checks no longer depend on it.

### `PT_STRING8` code page chain

The M3b design debt is closed in code. `PT_STRING8` bytes are decoded under a code page resolved once per message:

1. `PidTagMessageCodepage` (0x3FFD), when present and greater than zero. Zero means "use the folder's code page", which a standalone `.msg` cannot supply, so it counts as unspecified;
2. otherwise `PidTagInternetCodepage` (0x3FDE), when present and greater than zero. This ordering is a teaspoon design choice, not a quotation of a specification rule;
3. otherwise Windows-1252, reported as the fallback and never assumed silently.

The decoder implements Windows-1252, ISO-8859-1, US-ASCII, and UTF-8. Any other code page decodes only input that is entirely 7-bit ASCII, and only when the code page is a known ASCII superset; otherwise the value is reported under `variable_string8_unsupported_codepage_total` and no text is produced. This deliberately avoids a new dependency and avoids guessing. If real ANSI corpora call for East Asian or other multi-byte code pages, adding a full decoder (for example `encoding_rs`) is the next step and is a dependency decision for the project owner.

The chain is used wherever the extraction layer reads a string: the top-level message class, and the embedded message's class (each from that message's own properties stream).

**Evidence status:** covered by unit tests of the chain and the decoders, and by synthetic ANSI `.msg` files (a minimal CFB container built in a temp file at test time: a properties stream, optionally with the codepage properties, and a `PT_STRING8` message class stream). Not exercised by any real file: the corpus has none, so this path has not met a real producer's output. The first real ANSI `.msg` file is the actual verification.

### Corpus-gated regression test

`fixture_corpus_verify_is_clean` turns the M3e evidence from a one-time record into something re-runnable. It runs the same comparison `--verify` prints over the directory named by the `TSP_FIXTURE_DIR` environment variable, and asserts zero mismatches on every comparable field, zero on every structural gate, and a bounded RTF divergence of at most one file (the documented dictionary case). When the variable is unset it skips with a note; the fixtures are deliberately not in the repository, so CI cannot run it.

```text
$env:TSP_FIXTURE_DIR = "C:\dev\csr\main\teaspoon\_NOTES\test-fixtures\msgs"
cargo test fixture_corpus_verify_is_clean -- --nocapture
```

**Evidence status:** written, not yet run. Until it has been run on the corpus, treat it as unverified code. It should pass on the corpus for the reasons recorded above; a failure would be a finding, not a nuisance.

### Test count

56 at the M3f flip (verified). The follow-ups add eight tests (the chain, the decoders, unsupported code pages, two synthetic ANSI fixtures, two structural-gate tests, and the corpus-gated test, which counts as passing when skipped), for an expected 64. That number is a prediction until `cargo test` confirms it.

## Known limitations at close of M3

- `PT_STRING8` has been exercised only by synthetic fixtures; the corpus has no `PT_STRING8` property. Multi-byte and other unimplemented code pages are reported, not decoded.
- `msg_parser` exposes only To/Cc/Bcc; the custom path additionally reports ORIG and other/unresolved recipient buckets (all 0 on this corpus). After the default flip these are the MSG-side vocabulary, matching the PST side's.
- Embedded messages are opened one level deep only.
- The structural `--oxmsg` counters are whole-tree by design and not directly comparable to `msg_parser`'s outer-message-only counts without the top-level scoping used by the extraction layer.
- All MSG verification runs against one 29-file corpus, with `msg_parser` as the only oracle. No comparison against `libpff` or `libpst` has been run for either format.
- The two transitional flags (`--oxmsg`, `--extract`) are still in the binary until M3g.

## What M3g involves

Remove `--oxmsg` and `--extract`, and the code only they reach. The prerequisite the `--oxmsg` removal was waiting on — its accounting checks living somewhere else — is met by the `--verify` gates once the follow-ups above have run clean. Before removing the structural walk itself, note that `--verify` still calls it; what goes is the separate `--oxmsg` report (the entry-shape breakdown lines), which is the part with no other consumer.

---

# Retained M2.x groundwork evidence

The sections below are carried over from the previous `oxmsg-results.md` -- superseded by this file -- with only the terminology corrected where M3f made it stale; they remain the evidence base for the structural claims the M3 extraction layer builds on. Structural counters named here are now also reported by `tsp --verify`.

## Why the custom parser exists

`msg_parser` has no raw/generic property-iteration equivalent to `outlook-pst`'s `MessageProperties::get(id)`/`.iter()`. That is the one structural inconsistency remaining between teaspoon's two format adapters, and it is directly at odds with design principle 4 (unknown/unmapped properties are preserved or reported, not silently discarded) and ADR: loss-aware-normalized-representation. The PST side can tell you a property existed but could not be read; the MSG side, via `msg_parser` alone, cannot make that distinction for anything outside the specific fields the crate's `Outlook` struct happens to expose.

This was raised as a live decision once the original condition for deferring it — wait until loss-accounting requirements for MSG are concrete — was met through real evidence. `msg_parser`'s embedded-message-opening capability, one of the stronger original arguments for choosing it, was shown not to work in practice (see M2c, `docs/verification/m2-results.md`), while its core detection logic (`html_from_rtf()`) had already been shown unreliable and replaced with teaspoon's own spec-correct check.

## Dependency

`cfb` v0.15 (crates.io, MIT) is the mature generic reader for MS-CFB (Compound File Binary) containers. It handles the generic container layer; teaspoon supplies only the MS-OXMSG-specific naming-convention layer. It knows nothing about MAPI, properties, or message semantics. (An earlier version of this file said v0.14; `Cargo.toml` pins 0.15.)

## Structural enumeration and entry accounting

The `--oxmsg` path opens each `.msg` file with `cfb::open` and classifies every entry name against well-known MS-OXMSG conventions — never by inspecting content: `__properties_version1.0`, `__substg1.0_PPPPTTTT` (including the `-NNNNNNNN` multiple-value index suffix), `__attach_version1.0_#*` / `__recip_version1.0_#*` storages, `__nameid_version1.0`, and `__substg1.0_3701000D` embedded-object storages. Anything else is counted, never silently dropped, with a privacy-safe breakdown (CFB object kind, depth, name shape, fixed-vocabulary ancestry).

The 29-file corpus confirmed complete accounting. The block below is the first full-corpus report, before the unrecognized entries were resolved as described in the next section; it is kept as the record of that run:

```text
total_entries=2647
root_entries_total=29
properties_stream_entries_total=97
recognized_entries_total=2585
unrecognized_entries_total=62
entry_accounting_gap_total=0
```

The original 60-entry single-fixture discrepancy was a comparison artifact (a per-file presence counter mistaken for an entry count), resolved by the corpus run.

### Resolution of the 62 unrecognized entries

| Earlier finding | Count | Resolution |
|---|---|---|
| depth-1 `malformed_property_stream_name` streams | 56 | Standard multiple-valued value-stream names (`__substg1.0_PPPPTTTT-NNNNNNNN`), recognized. Corpus shows `indexed_property_streams_total=56`. |
| `stream_name_is_storage` mismatches | 2 | The two `__substg1.0_3701000D` embedded-object storages, classified as storages. |
| depth-3 `other_name` streams | 6 | Direct children of the single **custom** embedded-object storage — Word's own OLE streams (storage CLSID `f4754c9b-64f5-4b40-8af4-679732ac0607`, `Word.Document`), outside MS-OXMSG naming per "Custom Attachment Storage". Counted as `opaque_payload`, structurally accounted for, contents not interpreted. |

### The two embedded-object storages

| Storage | Shape | CLSID | Cross-check |
|---|---|---|---|
| 1 | message-shaped (directly contains `__properties_version1.0`) | nil | `msg_parser` reports `attachments_method_embedded_message=1` |
| 2 | custom (no `__properties_version1.0`) | Word.Document | `msg_parser` reports `attachments_method_ole=1` |

The counts agree. Independently, the property-level `PidTagAttachDataObject` Reserved-field cross-check agrees exactly: one entry reads `reserved=0x01` (embedded message), one reads `reserved=0x04` (storage), and `attach_data_object_reserved_vs_embedded_object_storage_gap=0` / `..._custom_object_storage_gap=0`, with `attach_data_object_size_sentinel_mismatches=0`. Two independent signals, same conclusion.

### Internal consistency

- Categories sum to the total: 29 root + 97 properties streams + 2417 property streams + 30 attachment + 37 recipient + 29 named-property + 2 embedded-object = 2641, plus 6 opaque = 2647.
- Properties streams by scope: 29 message + 37 recipient + 30 attachment + 1 embedded-object = 97.
- Named-property storages: 29, one per file, none inside the embedded message — consistent with embedded messages sharing the top-level name mapping per MS-OXMSG 2.2.3.
- `properties_stream_bytes_total=53504` = 52016 bytes of entries past headers = 3251 × 16, consistent with 16-byte fixed-length property entries.

### Property-entry decoding (structural)

```text
properties_entries_total=3251
properties_entries_fixed_inline_total=1221
properties_entries_variable_single_total=2019
properties_entries_variable_multivalued_total=11
properties_stream_too_short_for_header_total=0
properties_stream_trailing_bytes_total=0
properties_stream_unexpected_scope_total=0
properties_stream_read_errors=0
```

The three shape totals sum to `properties_entries_total` exactly (checked per scope against the `property_type` histogram). The property-type histogram also confirms `0x0048` (PT\_CLSID) is correctly classified as variable — GUIDs don't fit the entry's 8-byte inline value field.

## Named-property bit-layout investigation (M3c's foundation)

Named-property resolution initially surfaced a real anomaly (298 of 436 occurrences resolving to an undefined GUID index, none as string-named). Root cause: the Entry Stream's Index-and-Kind field was decoded backwards on every axis — which 16-bit half holds Property Index vs. GUID Index/Kind, and within the GUID/Kind half, which bit is Kind. Found by byte-level decoding of real fixture data (the same method that resolved the RTF-in-HTML ambiguity), not by further inference from spec text or crate source: file 0's GUID stream position 0 decodes to `00062008-0000-0000-C000-000000000046` (PSETID\_Common) exactly, and applying the corrected formula to file 9's entries whose GUID resolves to PSETID\_Task yields LIDs `0x8101` and `0x8102` — `PidLidTaskStatus` and `PidLidPercentComplete`, MS-OXPROPS' own canonical names for exactly that pair. Confirmed the same way across four files, sixteen entries, zero exceptions on the Property Index cross-check.

The earlier "138 resolved" entries under the old code were not real matches — `guid_index` was being read from Property Index, so any entry at array position 1 or 2 spuriously resolved as PS\_MAPI/PS\_PUBLIC\_STRINGS purely because those sentinel values coincide with common early array positions.

Adding `PS_INTERNET_HEADERS` (`{00020386-0000-0000-C000-000000000046}`) to the well-known property-set table split the `custom` bucket exactly as predicted: `custom` dropped from 250 to 143, with 107 correctly labeled `PS_INTERNET_HEADERS` — confirming with real data that internet-header named properties (string-named by construction) were the dominant contributor to what had looked like a large "unknown" bucket. The corrected layout is now permanently cross-checked (`named_properties_index_mismatch_total`).
