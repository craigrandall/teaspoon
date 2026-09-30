# teaspoon (tsp)

Standalone Rust tooling for deterministic, loss-aware mining of Outlook `.pst` and `.msg` files.

Shorthand for teaspoon (i.e. the name of this project) is tsp (i.e. the name of this project's tool), which is "pst" backwards. (ツ)

`tsp` inspects Outlook mail containers (`.pst` files, single `.msg` files, or a directory of `.msg` files) and prints a **privacy-safe inventory**: counts and structural facts only. It never prints message bodies, subject lines, sender/recipient addresses, attachment names or bytes, folder names, file paths, or any other free-form content. Its output is designed to be safe to share and safe to diff across versions.

## Current status

**M0 — architecture/research baseline:** established. All seven ADRs are Accepted. ADR "Custom MS-OXMSG parser graduates to the production MSG path" moved from Proposed to Accepted when M3f shipped. ADR "Independent differential verification" now records the MSG-side verification actually performed; the PST-side comparison against an independent implementation has not been run.

**M1 — PST feasibility spike: complete**, within the limits of `outlook-pst` v1.2.0's public API. A small read-only CLI (`tsp`) exercises the Microsoft Rust PST implementation: open a PST, reach the message store/IPM subtree, traverse folders, enumerate messages, and inspect raw message properties, plus aggregate message-class, body-availability, recipient-count/type, and attachment-count/classification diagnostics — all without emitting any message content.

- P1 through P4b are done and verified on Windows against a real PST fixture deliberately enhanced to cover plain/RTF bodies, a BCC recipient, an embedded-message attachment, and an OLE attachment — see `docs/verification/m1-results.md`.
- Zero-byte and by-reference attachments were explicitly excluded as fixture goals (empirically impractical to compose / obsolete in modern email) — see `docs/verification/m1-results.md` for the documented rationale. The classification code for both remains.
- Opening/traversing embedded-message or OLE attachment *content* was investigated (P4c) and found not achievable through `outlook-pst` v1.2.0's public API — accepted as M1's practical ceiling, not a defect. `tsp` correctly detects and counts these attachments on the PST side; it can't open them.
- HTML-in-RTF detection (MS-OXRTFEX `\fromhtml1` encapsulation) is implemented and confirmed correct against real data — see `docs/verification/m1-results.md`.

This is deliberately **not** the production miner and does not yet emit Markdown or extract body/attachment content.

**M2 — MSG ingestion spike: complete.** `tsp` dispatches on its input: a `.pst` file uses the PST path; a single `.msg` file or a directory of `.msg` files (scanned non-recursively) uses the MSG path. M2 built that path on the `msg_parser` crate, mirroring M1's structure. Body-type detection, recipient/attachment classification, and the zero-byte/subdirectory-visibility fixes were all confirmed correct against real data. Opening an embedded-message attachment as a nested message (M2c) was investigated across two independent real attempts and found not achievable through `msg_parser`, for a root cause never identified. That ceiling was later removed by the custom parser (M3e). See `docs/verification/m2-results.md` for the full evidence trail.

**M3 — MSG value extraction and default-path graduation: complete.** Stages M3a through M3f, and the follow-ups after the flip, are verified on Windows against the full 29-file corpus. M3g (removing the transitional flags) is written and awaiting its first build and test run. The custom MS-OXMSG parser decodes real property values — not just structure — and is now the **default** `.msg` path. The differential verification against `msg_parser` shows parity on every comparable output field, plus two documented, spec-backed improvements:

- **Fixed-value interpretation (M3a):** every fixed base type (`PT_SHORT` through `PT_SYSTIME`) decodes to its real typed value; boolean-encoding and non-finite-float checks both read 0 on the corpus.
- **Variable-length values (M3b):** `PT_UNICODE` (UTF-16LE), `PT_BINARY`, `PT_CLSID` (validated at 16 bytes), and `PT_STRING8` decode with zero anomalies on the corpus — size mismatches, odd UTF-16 lengths, decode errors, wrong-length CLSIDs all 0.
- **Named properties (M3c):** all 436 corpus occurrences resolve to a property set and kind with both cross-check gates at 0, following the bit-layout investigation that corrected the spec text against real fixture bytes. String-kind names decode successfully; names are read in memory but never printed.
- **Diagnostic rebuilt from decoded values (M3d):** the custom path prints the same report shape the `msg_parser` default used to, including a matching `attachments_zero_byte` count (zero bytes are counted only for confirmed-empty `PidTagAttachDataBinary` streams, with unreadable streams reported separately via `attachments_data_stream_missing`).
- **Differential verification (M3e):** `--verify` and the textual diff show parity on every comparable field, with exactly two differences, both triaged against the specifications rather than assumed in either direction: (1) `msg_parser`'s preset LZFu dictionary diverges from MS-OXRTFCP's published dictionary (the custom path is the correct one, differing by −1 decompressed byte on one file of 29), and (2) `msg_parser`'s `Attachment::as_message()` returns `None` on this corpus's one embedded-message attachment — the M2c ceiling — while the custom path opens it and reads its message class successfully.
- **Default flipped (M3f):** `.msg` input uses the custom path by default. The default run over the 29 fixtures produced output identical to the earlier `--extract` output, the build was clean, and all tests passed.

- **After the flip (verified):** the build was clean, all 64 tests passed, and the full-corpus `--verify` run reported parity on every comparable field and `structural_gate_violations=0`. Three follow-ups are part of that build:
  - The structural gates that previously ran only under `--oxmsg` (`entry_accounting_gap_total`, unrecognized-entry counts, and the other zero-expected structural counters) are reported by `--verify` under a single `structural_gate_violations` count.
  - `PT_STRING8` decoding is code page aware: `PidTagMessageCodepage`, then `PidTagInternetCodepage`, then a Windows-1252 fallback, with unimplemented code pages reported rather than guessed. On the corpus the chain resolved from a real code page property on all 29 files and never used the fallback. The corpus still has no `PT_STRING8` value, so the decoders themselves are exercised only by synthetic fixtures.
  - A corpus-gated regression test re-runs the both-paths comparison and the structural gates on demand (see [Build and run](#build-and-run)). It skips unless `TSP_FIXTURE_DIR` is set, and the suite passing does not record that it ran against the corpus.
- **M3g (written, not yet built or run):** `--oxmsg` and `--extract` were removed; passing either is now a command-line error. The structural breakdown that `--oxmsg` printed is now printed by `--verify` only when a structural gate is nonzero. Expected: 66 tests, `--verify` output unchanged on the corpus.

See `docs/verification/m3-results.md` for the full evidence trail, including the run commands and the field-by-field diff table.

`msg_parser` remains a normal (non-dev) dependency, because the `--verify` differential harness ships in the binary. It is the **oracle only**: no default code path calls it, and there is deliberately no runtime fallback flag.

## Design principles

1. `.pst` and `.msg` are input formats, not the domain model.
2. Format adapters produce a common Outlook-item representation.
3. Markdown is a projection, not the canonical representation.
4. Unknown/unmapped properties are preserved or reported rather than silently discarded.
5. Extraction loss is explicit.
6. Source provenance is part of the output model.
7. Evidence distinguishes specification support, implementation support, tests, and independent verification.

## Build and run

```
cargo run --release -- .\sample.pst
cargo run --release -- .\folder-of-msgs\
cargo run --release -- .\sample.msg
cargo run --release -- --verify .\folder-of-msgs\    # differential check against msg_parser + structural gates
cargo run --release -- --verify .\sample.msg
```

`cargo test` runs the unit tests: classification, decoding (including the `PT_STRING8` code page chain and synthetic ANSI `.msg` fixtures built at test time), counters, every verify-comparison outcome, and the structural gates.

One test is gated on a local corpus. Real `.msg` fixtures are deliberately not in this repository, so `fixture_corpus_verify_is_clean` runs only when `TSP_FIXTURE_DIR` points at a directory of `.msg` files, and otherwise skips with a note:

```
$env:TSP_FIXTURE_DIR = "C:\path\to\msgs"
cargo test fixture_corpus_verify_is_clean -- --nocapture
```

It asserts zero mismatches on every comparable field and zero on every structural gate, allowing exactly the documented RTF-dictionary divergence. Failure messages carry counts only, never content.

Input may be:

- a `.pst` file — full PST diagnostic,
- a `.msg` file — MSG diagnostic through the custom MS-OXMSG parser,
- a directory — every `.msg` file directly inside it is scanned  
(non-recursive; skipped subdirectories are reported as  
`subdirectories_skipped`).

Real PST/MSG fixtures are required to perform the behavioral portion of any milestone. No personal mail data is embedded in this repository.

CI (`.github/workflows/ci.yml`) runs format, check, clippy (`-D warnings`), test, and a release build on Linux and Windows. It cannot exercise real PST/MSG parsing, because the fixtures are excluded from the repo, so the corpus-gated test and the fixture runs remain a manual step on a machine that has the fixtures.

## Modes

| Command | What it does |
|---|---|
| `tsp <input>` | Default diagnostic: PST input uses the PST path; `.msg` input uses the custom MS-OXMSG extraction path. |
| `tsp --verify <msg input>` | Differential verification: runs the custom extraction path and `msg_parser` (the oracle) over the same files and prints match/mismatch counts per field — never the values compared — followed by the custom path's structural gate counters and `structural_gate_violations`. When any gate is nonzero, a `structural_breakdown=follows` marker and the privacy-safe structural breakdown are printed as well, to make the violation triageable. |

The transitional `--oxmsg` (structural diagnostic) and `--extract` (alias of the default) flags were removed in M3g. Passing either is a command-line error.

## Output format

Reports are `key=value` lines (plus grouped `key field=value` lines for per-class / per-scope breakdowns). Keys are a **stable output vocabulary**: they are unchanged across versions so that outputs from different versions and modes can be diffed mechanically. The default `.msg` report is identical in shape to the report the `msg_parser`-based default produced before M3f, with one deliberate extra key (`attachments_data_stream_missing`, a custom-path-only anomaly signal; 0 on the verification corpus).

Every counter is either an aggregate count, a bounded MAPI vocabulary value (message classes, property IDs/types, attach methods, recipient types, property-set labels, CLSIDs, code page numbers), or a size in bytes — never user content. Anything the tool cannot classify is counted (`*_unrecognized*`, `*_errors`, `*_unknown`, `entry_accounting_gap_total`, `*_unsupported_codepage*`) rather than silently dropped, so nothing disappears without a trace.

## Architecture (single `main.rs`, top to bottom)

1. **CLI and input classification** — `Args`, `InputKind`, `classify_input`, and the dispatch in `main()`.
2. **Shared detection and vocabulary** — the MS-OXRTFEX `\fromhtml1` encapsulated-HTML check (`check_compressed_rtf_bytes`), MAPI property / recipient-type / attach-method constants, and the shared counters (`BodyCounters`, `CountStats`, `ZeroByteStats`) used identically by the PST and MSG paths so the two adapters cannot drift apart.
3. **PST diagnostic** — walks the IPM subtree via `outlook_pst` and counts message classes, body availability (plain / native HTML / HTML encapsulated in RTF), recipient types, and attachment methods.
4. **MSG report** — the shared report shape (`MsgTotals`, `print_msg_report`), populated by the custom path.
5. **Custom MS-OXMSG parser** — pure, string-based classification of the CFB naming conventions (`classify_oxmsg_entry` and friends), property stream/value decoding primitives (fixed-value decoding to real typed values, UTF-16LE decoding, the `PT_STRING8` code page chain and decoders), the structural walk (two passes: classify entries, then decode properties streams), run by `--verify`, the named-property map resolution, and the extraction layer (`extract_*`, `open_embedded_message`, `run_msg_extract`) that feeds both the default report and `--verify`.
6. **Differential verification** — comparison types, `collect_msg_verify_totals`, `print_msg_verify_report`, the structural gates (`structural_gate_values`), and the breakdown printed when a gate fires (`print_structural_breakdown`).
7. **Tests** — classification, decoding, counters, every verify-comparison outcome, synthetic ANSI fixtures, structural gates, and the corpus-gated regression test.

External crates: `outlook-pst` (PST), `cfb` (generic MS-CFB container), `compressed-rtf` (MS-OXRTFCP), `msg_parser` (oracle for `--verify` only), `clap`, `anyhow`.

## Privacy design rules

- Never print input paths, display names, subjects, bodies, addresses, attachment filenames, or entry IDs — including in error messages.
- Message-class names, property IDs/types, CLSIDs, and code page numbers are treated as bounded vocabularies, not content, and may be reported by value.
- `--verify` and the default extraction read real content into memory to compute booleans and match counts, but never print it.
- Loss must be explicit: every unreadable, unrecognized, or unresolvable item increments its own counter.

## Known limitations

- Neither M1 nor M2 nor M3 yet proves complete extraction fidelity. Markdown rendering, attachment byte preservation, named-property normalization (decoding and resolution verified on the MSG side; nothing yet feeds the normalized model), and body extraction remain future work.
- `msg_parser` exposes only To/Cc/Bcc, so an ORIG-classified recipient (sender, MS-OXOMSG value 0) cannot be detected through it; the PST side and the custom path both track it.
- `PT_STRING8` decoding implements Windows-1252, ISO-8859-1, US-ASCII, and UTF-8. Any other code page decodes only all-ASCII input (and only for a known ASCII-superset code page) and is otherwise reported as unsupported; no text is produced. The corpus contains no `PT_STRING8` value, so decoding has been exercised only by synthetic fixtures. (Code page *resolution* has met real data: all 29 corpus files resolved from a real code page property.) See `docs/verification/m3-results.md`.
- PST table reads assume `PidTagRecipientType` / `PidTagAttachMethod` / `PidTagAttachSize` arrive as 32-bit integers; a different encoding falls into the "unknown" bucket rather than miscounting, but the assumption has never been falsified by contrary data.
- Embedded messages are opened one level deep only. The PST side cannot open them at all (P4c).
- The corpus-gated regression test is not recorded as having run against the corpus; the suite passing includes it skipping. Run it once with `TSP_FIXTURE_DIR` set.
- All MSG verification runs against one 29-file corpus, with `msg_parser` as the only oracle. No comparison against an independent PST implementation, or against `libpff`/`libpst` for either format, has been run.

## Change history

Background dates below refer to the project's own verification timeline; the underlying facts are documented in more detail in the `docs/verification/` results files in the teaspoon project.

- **2026-09-07** — MSG-side HTML detection fix: `msg_parser`'s `html_from_rtf()` non-emptiness was being trusted as "HTML was found".
- **2026-09-13** — Correction: that trust was proven wrong. `RTF_message.msg` (confirmed genuinely RTF-authored via Outlook's View Source: an explicit "Converted from text/rtf format" comment and an "MS Exchange Server" generator tag) still produced non-empty output from `html_from_rtf()`. Both the PST and MSG diagnostics now check the decompressed RTF bytes directly for the MS-OXRTFEX `\fromhtml1` control word — the specification-sanctioned signal — instead of either crate's convenience method. Same date: confirmed the plain-text body counter reflects a compatibility mirror Outlook populates regardless of authoring format, and that many real messages carry HTML only inside `PidTagRtfCompressed`.
- **2026-09-14** — Added `rtf_decompressed_bytes_total` (size-only) so the PST and MSG sides can be compared without content; comparative analysis of the two format adapters recorded in project correspondence.
- **MS-OXMSG bit-layout investigation** — the Named Property entry stream's Property Index / GUID Index / Kind bit layout was confirmed against real fixture bytes, after both the spec text and a partially-read crate source proved wrong; the confirmed layout is now permanently cross-checked (`named_properties_index_mismatch_total`). See `docs/verification/m3-results.md`.
- **MS-OXRTFCP dictionary investigation** — `msg_parser`'s preset LZFu dictionary was verified against Microsoft's published MS-OXRTFCP dictionary and found to diverge; `compressed-rtf` (the custom path's dependency) matches the spec exactly. The single-file, −1-byte disagreement this produces is documented in `docs/verification/m3-results.md`.
- **M3 extraction layer** — fixed-value decoding (all ten fixed base types), variable-length value reading (UTF-16LE / Windows-1252 / CLSID), named-property value resolution, the `--verify` differential harness, and the default-shaped report all landed and were verified against the 29-file corpus; the zero-byte-attachment detection gap between the two MSG paths closed (unreadable data streams now reported separately via `attachments_data_stream_missing` instead of counted as empty). Full evidence in `docs/verification/m3-results.md`.
- **Refactor (no behavior or output changes)** — code reorganized for one-job-per-function (SLAP): the RTF encapsulated-HTML check unified into `check_compressed_rtf_bytes` shared by the PST, MSG, and custom paths; the three copies of the "top-level storage" CFB walk filter unified into `top_level_storage_paths`; a `CompoundFile` type alias replaces the repeated generic spelling; the `__substg1.0_3701000D` literal replaced by a named constant; `CollectedOxmsgEntry` moved next to the walk that uses it; long dated comment narratives condensed into this file. Fixed a stale doc comment on `record_attachment_content_id_presence` that still contained the truncated remains of a removed zero-size-recording function's documentation, and applied one clippy fix (`matches!` macro in `top_level_storage_paths`) flagged by `-D warnings`.
- **Embedded-message opening corrected** — the custom path now opens each embedded-message attachment through that attachment's own attach method and reads the nested message's own class. An earlier draft could read the outer message's class and overcount success. The corpus run then showed `embedded_messages_opened=1` genuinely.
- **2026-09-29 (M3f)** — Default flipped: `.msg` input uses the custom MS-OXMSG path. The `msg_parser`-based default report path and its helpers were removed, and `msg_parser` is used only by `--verify`. The default run over the 29 fixtures matched the earlier `--extract` output exactly; the build was clean and the tests passed.
- **2026-09-29 (post-M3f)** — Structural gate counters folded into `--verify` (`structural_gate_violations`); `PT_STRING8` code page chain and synthetic ANSI fixtures added; the corpus-gated both-paths regression test added. Verified on Windows: clean build, 64 tests passing, and a full-corpus `--verify` run with parity on every comparable field and `structural_gate_violations=0`. The code page chain resolved without its fallback on all 29 files (3 from the message code page, 26 from the Internet code page).
- **2026-09-29 (M3g)** — Removed the transitional `--oxmsg` and `--extract` flags and `run_oxmsg_diagnostic`. The structural breakdown is now printed by `--verify` only when a structural gate is nonzero. Written; awaiting its first Windows build and test run (expected 66 tests). Documentation and ADRs revised to match.
