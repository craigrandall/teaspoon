# teaspoon (tsp)

Standalone Rust tooling for deterministic, loss-aware mining of Outlook `.pst` and `.msg` files.

Shorthand for teaspoon (i.e. the name of this project) is tsp (i.e. the name of this project's tool), which is "pst" backwards. (ツ)

`tsp` inspects Outlook mail containers (`.pst` files, single `.msg` files, or a directory of `.msg` files) and prints a **privacy-safe inventory**: counts and structural facts only. It never prints message bodies, subject lines, sender/recipient addresses, attachment names or bytes, folder names, file paths, or any other free-form content. Its output is designed to be safe to share and safe to diff across versions.

## Current status

**M0 — architecture/research baseline:** established. Five of six ADRs are Accepted; ADR-0006 (independent differential verification) remains Proposed until that verification work actually happens.

**M1 — PST feasibility spike: complete**, within the limits of `outlook-pst` v1.2.0's public API. A small read-only CLI (`tsp`) exercises the Microsoft Rust PST implementation: open a PST, reach the message store/IPM subtree, traverse folders, enumerate messages, and inspect raw message properties, plus aggregate message-class, body-availability, recipient-count/type, and attachment-count/classification diagnostics — all without emitting any message content.

- P1 through P4b are done and verified on Windows against a real PST fixture deliberately enhanced to cover plain/RTF bodies, a BCC recipient, an embedded-message attachment, and an OLE attachment — see `docs/verification/m1-results.md`.
- Zero-byte and by-reference attachments were explicitly excluded as fixture goals (empirically impractical to compose / obsolete in modern email) — see `docs/verification/m1-results.md` for the documented rationale. The classification code for both remains.
- Opening/traversing embedded-message or OLE attachment *content* was investigated (P4c) and found not achievable through `outlook-pst` v1.2.0's public API — accepted as M1's practical ceiling, not a defect. `tsp` correctly detects and counts these attachments; it can't open them.
- HTML-in-RTF detection (MS-OXRTFEX `\fromhtml1` encapsulation) is implemented and confirmed correct against real data — see `docs/verification/m1-results.md`.

This is deliberately **not** the production miner and does not yet emit Markdown or extract body/attachment content.

**M2 — MSG ingestion spike: complete.** `tsp` dispatches on its input: a `.pst` file uses the unchanged M1 path; a single `.msg` file or a directory of `.msg` files (scanned non-recursively) uses a diagnostic built on the `msg_parser` crate, mirroring M1's structure. Body-type detection, recipient/attachment classification, and the zero-byte/subdirectory-visibility fixes are all confirmed correct against real data. Opening an embedded-message attachment as a nested message (M2c) was investigated across two independent real attempts and found not achievable in practice through `msg_parser`, for a root cause not identified despite repeated research — an accepted ceiling at the time, mirroring P4c on the PST side. See `docs/verification/m2-results.md` for the full evidence trail.

**M3 — MSG value extraction and default-path graduation: stages M3a through M3e complete and verified against the full 29-file corpus; M3f (flip the default) is the next decision, with its evidence gate satisfied.** The custom MS-OXMSG parser now decodes real property values — not just structure — and the differential verification against `msg_parser` shows parity on every comparable output field, plus two documented, spec-backed improvements:

- **Fixed-value interpretation (M3a):** every fixed base type (`PT_SHORT` through `PT_SYSTIME`) decodes to its real typed value; boolean-encoding and non-finite-float checks both read 0 on the corpus.
- **Variable-length values (M3b):** `PT_UNICODE` (UTF-16LE), `PT_BINARY`, `PT_CLSID` (validated at 16 bytes), and `PT_STRING8` (Windows-1252) decode with zero anomalies — size mismatches, odd UTF-16 lengths, decode errors, wrong-length CLSIDs all 0. The `PT_STRING8` decoder remains unexercised by real data (the corpus has none) and does not yet consult `PidTagMessageCodepage` — documented design debt in `docs/verification/m3-results.md`.
- **Named properties (M3c):** all 436 corpus occurrences resolve to a property set and kind with both cross-check gates at 0, following the bit-layout investigation that corrected the spec text against real fixture bytes. String-kind names decode successfully; names are read in memory but never printed.
- **Diagnostic rebuilt from decoded values (M3d):** `--extract` runs the custom path alone and prints the exact same report shape as the default `.msg` diagnostic, byte-diffable against it — including a matching `attachments_zero_byte` count, closing the previously documented zero-byte-attachment detection gap (zero bytes are now counted only for confirmed-empty `PidTagAttachDataBinary` streams, with unreadable streams reported separately via `attachments_data_stream_missing`).
- **Differential verification (M3e):** `--verify` and the `--extract` textual diff show parity on every comparable field, with exactly two differences, both triaged against the specifications rather than assumed in either direction: (1) `msg_parser`'s preset LZFu dictionary diverges from MS-OXRTFCP's published dictionary (verified against Microsoft's own spec text; the custom path is the correct one, differing by −1 decompressed byte on one file of 29), and (2) `msg_parser`'s `Attachment::as_message()` returns `None` on this corpus's one embedded-message attachment — the M2c ceiling — while the custom path opens it and reads its message class successfully.

See `docs/verification/m3-results.md` for the full evidence trail, including the run commands and the field-by-field diff table. M3f — changing `main()`'s dispatch so `.msg` input uses the custom path by default — is the next step, and M3g (retiring the `--oxmsg` flag) follows only after the flipped default has real runs behind it.

`msg_parser` remains a dev-dependency for the `--verify` differential harness; the custom path is its replacement for runtime extraction, not yet the default.

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
cargo run --release -- --oxmsg .\folder-of-msgs\     # structural diagnostic
cargo run --release -- --verify .\folder-of-msgs\    # differential check against msg_parser
cargo run --release -- --extract .\folder-of-msgs\   # custom path's own default-shaped report
cargo run --release -- .\sample.msg
cargo run --release -- --oxmsg .\sample.msg          # structural diagnostic
cargo run --release -- --verify .\sample.msg         # differential check against msg_parser
cargo run --release -- --extract .\sample.msg        # custom path's own default-shaped report
```

`cargo test` runs the unit tests (57 covering classification, decoding, counters, and every verify-comparison outcome).

Input may be:

- a `.pst` file — full PST diagnostic,
- a `.msg` file — msg\_parser-based MSG diagnostic,
- a directory — every `.msg` file directly inside it is scanned  
(non-recursive; skipped subdirectories are reported as  
`subdirectories_skipped`).

Real PST/MSG fixtures are required to perform the behavioral portion of any milestone. No personal mail data is embedded in this repository.

## Modes


| Command                     | What it does                                                                                                                                                                                                                           |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tsp <input>`               | Default PST or MSG diagnostic (per input type).                                                                                                                                                                                        |
| `tsp --oxmsg <msg input>`   | Custom MS-OXMSG parser: structural enumeration of the CFB container via the `cfb` crate. Reports names/counts/sizes only; PST input is unaffected.                                                                                     |
| `tsp --verify <msg input>`  | Differential verification: runs both the custom MS-OXMSG extraction path and `msg_parser` over the same files and prints match/mismatch counts per field — never the values compared. Takes precedence over `--oxmsg` and `--extract`. |
| `tsp --extract <msg input>` | Runs the custom extraction path alone (no `msg_parser`) and prints the same report shape as the default `.msg` diagnostic, meant to be textually diffed against it. Takes precedence over `--oxmsg`.                                   |


## Output format

Reports are `key=value` lines (plus grouped `key field=value` lines for per-class / per-scope breakdowns). Keys are a **stable output vocabulary**: they are unchanged across versions so that outputs from different versions and modes can be diffed mechanically. `--extract` output is byte-diffable against the default `.msg` report, with exactly one deliberate extra key (`attachments_data_stream_missing`, a custom-path-only anomaly signal; 0 on the verification corpus).

Every counter is either an aggregate count, a bounded MAPI vocabulary value (message classes, property IDs/types, attach methods, recipient types, property-set labels, CLSIDs), or a size in bytes — never user content. Anything the tool cannot classify is counted (`*_unrecognized*`, `*_errors`, `*_unknown`, `entry_accounting_gap_total`) rather than silently dropped, so nothing disappears without a trace.

## Architecture (single `main.rs`, top to bottom)

1. **CLI and input classification** — `Args`, `InputKind`, `classify_input`.
2. **Shared detection and vocabulary** — the MS-OXRTFEX `\fromhtml1` encapsulated-HTML check (`check_compressed_rtf_bytes`), MAPI property / recipient-type / attach-method constants, and the shared counters (`BodyCounters`, `CountStats`, `ZeroByteStats`) used identically by the PST and MSG paths so the two adapters cannot drift apart.
3. **PST diagnostic** — walks the IPM subtree via `outlook_pst` and counts message classes, body availability (plain / native HTML / HTML encapsulated in RTF), recipient types, and attachment methods.
4. **MSG diagnostic** — the same report shape, sourced from `msg_parser`.
5. **Custom MS-OXMSG parser** — pure, string-based classification of the CFB naming conventions (`classify_oxmsg_entry` and friends), property stream/value decoding primitives (including fixed-value decoding to real typed values and UTF-16LE/Windows-1252 string decoding), the `--oxmsg` structural walk (two passes: classify entries, then decode properties streams), the named-property map resolution, and the real extraction layer used by `--verify` and `--extract`.
6. **Differential verification** — comparison types and `run_msg_verify`.
7. **Tests** — 57 unit tests covering classification, decoding, counters, and every verify-comparison outcome.

External crates: `outlook-pst` (PST), `msg_parser` (MSG), `cfb` (generic MS-CFB container), `compressed-rtf` (MS-OXRTFCP), `clap`, `anyhow`.

## Privacy design rules

- Never print input paths, display names, subjects, bodies, addresses, attachment filenames, or entry IDs — including in error messages.
- Message-class names, property IDs/types, and CLSIDs are treated as bounded vocabularies, not content, and may be reported by value.
- `--verify` and `--extract` read real content into memory to compute booleans and match counts, but never print it.
- Loss must be explicit: every unreadable, unrecognized, or unresolvable item increments its own counter.

## Known limitations

- Neither M1 nor M2 yet proves complete extraction fidelity. Markdown rendering, attachment byte preservation, named-property normalization (decoding and resolution now verified on the MSG side; nothing yet feeds the normalized model), and body extraction remain future work.
- `msg_parser` exposes only To/Cc/Bcc, so an ORIG-classified recipient (sender, MS-OXOMSG value 0) cannot be detected through it; the PST side and the custom path both track it.
- The PT\_STRING8 decode assumes Windows-1252 and does not consult `PidTagMessageCodepage`; the verification corpus contains no PT\_STRING8 property, so it has never been exercised against real data — see `docs/verification/m3-results.md` for the documented design debt and planned codepage-resolution chain.
- PST table reads assume `PidTagRecipientType` / `PidTagAttachMethod` / `PidTagAttachSize` arrive as 32-bit integers; a different encoding falls into the "unknown" bucket rather than miscounting, but the assumption has never been falsified by contrary data.
- Embedded messages are opened one level deep only.
- The custom MS-OXMSG parser's evidence gate for becoming the default MSG path (parity plus documented improvements, per M3e) is satisfied; the dispatch flip itself (M3f) has not yet shipped.

## Change history

Background dates below refer to the project's own verification timeline; the underlying facts are documented in more detail in the `docs/verification/` results files in the teaspoon project.

- **2026-09-07** — MSG-side HTML detection fix: `msg_parser`'s `html_from_rtf()` non-emptiness was being trusted as "HTML was found".
- **2026-09-13** — Correction: that trust was proven wrong. `RTF_message.msg` (confirmed genuinely RTF-authored via Outlook's View Source: an explicit "Converted from text/rtf format" comment and an "MS Exchange Server" generator tag) still produced non-empty output from `html_from_rtf()`. Both the PST and MSG diagnostics now check the decompressed RTF bytes directly for the MS-OXRTFEX `\fromhtml1` control word — the specification-sanctioned signal — instead of either crate's convenience method. Same date: confirmed the plain-text body counter reflects a compatibility mirror Outlook populates regardless of authoring format, and that many real messages carry HTML only inside `PidTagRtfCompressed`.
- **2026-09-14** — Added `rtf_decompressed_bytes_total` (size-only) so the PST and MSG sides can be compared without content; comparative analysis of the two format adapters recorded in project correspondence.
- **MS-OXMSG bit-layout investigation** — the Named Property entry stream's Property Index / GUID Index / Kind bit layout was confirmed against real fixture bytes, after both the spec text and a partially-read crate source proved wrong; the confirmed layout is now permanently cross-checked (`named_properties_index_mismatch_total`). See `docs/verification/m3-results.md`.
- **MS-OXRTFCP dictionary investigation** — `msg_parser`'s preset LZFu dictionary was verified against Microsoft's published MS-OXRTFCP dictionary and found to diverge; `compressed-rtf` (the custom path's dependency) matches the spec exactly. The single-file, −1-byte disagreement this produces is documented in `docs/verification/m3-results.md`.
- **M3 extraction layer** — fixed-value decoding (all ten fixed base types), variable-length value reading (UTF-16LE / Windows-1252 / CLSID), named-property value resolution, the `--verify` differential harness, and the `--extract` default-shaped report all landed and were verified against the 29-file corpus; the zero-byte-attachment detection gap between the two MSG paths closed (unreadable data streams now reported separately via `attachments_data_stream_missing` instead of counted as empty). Full evidence in `docs/verification/m3-results.md`.
- **This revision (refactor)** — no behavior or output changes. Code reorganized for one-job-per-function (SLAP): the RTF encapsulated-HTML check unified into `check_compressed_rtf_bytes` shared by the PST, MSG, and custom paths; the three copies of the "top-level storage" CFB walk filter unified into `top_level_storage_paths`; a `CompoundFile` type alias replaces the repeated generic spelling; the `__substg1.0_3701000D` literal replaced by a named constant; `CollectedOxmsgEntry` moved next to the walk that uses it; long dated comment narratives condensed into this file. Fixed a stale doc comment on `record_attachment_content_id_presence` that still contained the truncated remains of a removed zero-size-recording function's documentation, and applied one clippy fix (`matches!` macro in `top_level_storage_paths`) flagged by `-D warnings`.
