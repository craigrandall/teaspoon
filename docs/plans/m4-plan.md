# M4 plan (v4.2) — normalized model and deterministic Markdown archive

Status: **M4a-1 decided; M4b-1, M4b-2, and M4b-3 verified (v0.1.22, 2026-10-02) with open items listed in the M4b-3 section; M4c built and passing its synthetic tests (tag v0.1.23.1, 2026-10-03), with its real-corpus gate still open.** Version 4.2, revised 2026-10-03 from version 4.1 to record the M4c build. Every stage has an evidence gate that must be met on Windows before the stage counts as done. Companion documents:

- [`m4a-export-rules.md`](m4a-export-rules.md): the draft naming, layout, identity, duplicate, path-length, and overwrite rules (v3.4, aligned with the accepted archive ADR).
- [`m4a-dependency-research.md`](m4a-dependency-research.md): RTF de-encapsulation, HTML-to-Markdown converters, and supporting crates.
- [`../verification/m4-results.md`](../verification/m4-results.md): the evidence recorded so far.

## Decisions in force (project owner, 2026-09-30)

- Staged order: module split first, walking skeleton before breadth.
- **Layout stays as the accepted ADR [Deterministic Markdown archive](../../ADRs/deterministic-markdown-archive.md) chose:** one folder per message containing `message.md`, `metadata.json`, and an `attachments/` subfolder. That ADR is not superseded. The message folder's name mirrors the message subject; `attachments/` is a literal name.
- Export mirrors the PST folder tree. Root is `<out>/<PST file stem>/` (sanitized) so several PSTs can share one `<out>`.
- Duplicates: one uniform, zero-padded suffix for folders, messages, and attachments. The first item in a group keeps the bare name; the k-th is ` (kk)`, at least two digits, widened per group beyond 99, in deterministic (time, identifier) order. There is no ` - Copy`; content identity is recorded in metadata (`content_sha256`, `identical_to`), never in names.
- Metadata lives alongside the item it describes: `metadata.json` in a message folder, `folder.json` in a folder directory.
- Replacement characters as in rule N3; scope: everything reachable from the IPM subtree, non-mail items as metadata stubs; inline attachments stay in `attachments/` and are marked `inline`.
- Overwrite safety: `tsp` warns (counts only) and asks permission, or exits.
- Path budget: default 259 units from the real root; `--long-paths` opt-in; `--max-relative-path N` for archives that will move; the longest relative path is recorded and reported.
- Folder marker files: `folder.json` in every folder directory.
- The source file's SHA-256 is recorded in the root `folder.json` by default; `--no-source-hash` skips it.
- Non-PST inputs: a directory of `.msg` files exports to `<out>/<directory name>/` and **mirrors its subdirectories as folders** (export only; the diagnostic's non-recursive scan is unchanged); a single `.msg` exports to `<out>/<msg file stem>/`.
- Path length is handled up front, using UTF-16 code unit counts and worst-case child reservation.
- Approved dependencies: `serde`, `serde_json`, `sha2`, `unicode-normalization`, and `proptest` (dev-only). The HTML-to-Markdown bake-off is approved.
- The owner will create the priority fixtures separately.
- 2026-10-03: the owner directed that M4c proceed ahead of M4a-2 (the ADRs) and M4b-4 (the model types), which this plan places before it. Neither is done; M4c used a small seed of the model and **draft** schemas so that the ADRs can freeze what the skeleton has actually produced.

## What changed from v4.1

1. M4c is built; its status, behavior, and the decisions taken in it are recorded in the M4c section.
2. Two statements in v4.1 were withdrawn: the claim that the fixture list was "unchanged" (nothing had established that), and the M4b-3 note that `--out` without `--dry-run` was unusable (true at v0.1.22; export exists since M4c).

## What changed from v4

M4b-1 to M4b-3 were recorded as verified with the evidence in `m4-results.md`, and the open items (PST counts, short-`--out` run) were listed.

## What changed from v3

1. **One duplicate suffix.** ` - Copy` is gone; ` (nn)` by position replaces it everywhere. This saves characters (reserve 6 units, not 12), stops mislabeling different messages as copies, and removes naming's dependency on the content-hash definition, so the pure naming engine and the census never hash content.
2. **Content identity is metadata-only.** The Q11 content-hash definition moves to M4g and no longer blocks anything earlier.
3. **Directory exports mirror subdirectories;** source-file SHA-256 by default; Q7-Q10 closed.

## What changed from v2

1. **Layout reverted to the accepted ADR.** v2 had proposed `<subject>.md` plus `<subject> - attachments/` and a superseding ADR. That is withdrawn. The new ADRs in M4a-2 *complete* the archive ADR (identity, sanitization, collisions, attachment relationships, which it deferred) and do not replace it.
2. **Metadata location.** No hidden `.tsp/` store. `metadata.json` is per message (as the ADR says) and `folder.json` is per folder (the folder marker Q8 asked about, with ownership and identity duties).
3. **Naming rules adapted** to directory names: no extension to split, device names apply to directories, the 248-character directory rule becomes the binding path constraint, `attachments/` cannot collide with siblings, and the attachments-path reservation is 13 units plus the longest attachment name.
4. **Long paths, folder markers, and other delegated questions answered** in the rules document (L7, section 3.2), and accepted by the owner in v4.

## Goal

Turn what the two adapters read into a durable archive: a Windows-safe directory tree mirroring the source's folders, one directory per message named from its subject, with `message.md`, `metadata.json`, and `attachments/`, plus a `folder.json` per folder carrying identity, provenance, and loss status. M4 is the first milestone where `tsp` writes message content to disk; stdout stays counts-only.

## Where the code stands (v0.1.23.1)

- The MSG adapter decodes typed properties, value streams, named properties, recipients, attachments (methods, content IDs, emptiness), and one level of embedded message. It reads content into memory and prints only counts.
- The PST adapter is a counter-only diagnostic over `outlook-pst` v1.2.0. It cannot open embedded-message or OLE attachments (P4c).
- `msg_parser` is an oracle for comparable MSG fields; no PST oracle has been run.
- The pure naming rules and export planner exist (`naming.rs`, `plan.rs`), and `--dry-run` plans an export of a PST, a `.msg`, or a directory of `.msg` files and prints a content-free census.
- **`tsp <msg|dir> --out <dir>` exports** `.msg` input: `message.md` (subject heading and plain-text body in a code fence), `metadata.json`, and `folder.json` files, with preflight, consent, staged writes, and read-back. The schemas are drafts (`0.1-draft`). A seed of the model exists (`model.rs`); the full model, the other bodies, the envelope, attachment bytes, and PST export do not. 155 tests pass.
- Fixtures: 29 `.msg` files at the top of the fixture directory (27 carry HTML only inside RTF; 23 attachments carry a content ID; 1 embedded message; 1 zero-byte attachment; no `PT_STRING8` value), more `.msg` files in 3 subdirectories (which the recursive dry run and export include), and one enhanced PST.

## Working rules (unchanged)

- Staged, each stage with an evidence gate; done means met on Windows.
- Claude drafts uncompiled; the owner runs the quality gate and supplies output. Docs are updated per stage and separate verified from written.
- Stdout is content-free at every stage. Archive content goes only to the directory the user names.
- Loss is explicit: never silently dropped.
- Symmetry between adapters where the PST side allows.
- Sources of truth: Microsoft's specifications for formats and Win32 naming; independent implementations as oracles, not authorities.

## Stages

### M4a-1 — draft decisions and rules (documentation)

Done: [`m4a-export-rules.md`](m4a-export-rules.md) and [`m4a-dependency-research.md`](m4a-dependency-research.md). Every question in the rules document is decided except Q11 (the exact content-hash definition), which is metadata-only and waits for M4g.

### M4b-1 — mechanical module split

Status: **verified (v0.1.20).** Clean build, 66 tests passing, and byte-identical output against the pre-split build (tag v0.1.19) on the 29-file `.msg` corpus and `tsp-tester.pst`: default report (850 bytes, 33 lines), `--verify` (2,705 bytes, 82 lines, `structural_gate_violations=0`), and the PST diagnostic (1,092 bytes, 44 lines), all identical, with empty stderr.

Move `main.rs` into modules along the seams the README documents: CLI, shared vocabulary, PST diagnostic, MSG report, custom parser (structure, decoding, extraction), verification. No behavior change.

Gate: default `.msg` output, `--verify` output, and PST output byte-identical before and after (saved reports diffed); `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and the same 66 tests passing.

### M4b-2 — pure naming and planning modules

Status: **verified.** Clean build and 109 tests passing (66 existing plus 43 new). Two new modules, `src/naming.rs` and `src/plan.rs`, with no I/O and no PST or MSG types. New dependencies: `unicode-normalization` and, dev-only, `proptest`.

- `naming`: `sanitize_component` (N2-N8), `is_valid_component`, `split_extension` (N10), `is_special_attachment_name` (N6), `shorten_to` (L5), `collision_key` (U1), the suffix helpers, and `assign_namespace` (U2-U5: natural names claim first, folders before messages, then time and identifier, zero-padded ` (nn)` widened per group, reserved names such as `folder.json`).
- `plan`: `plan_export(tree, policy)` plans a whole tree (folders, message directories, `attachments/` files, embedded messages up to the depth cap) against the 259-unit path and 247-unit directory limits with worst-case child reservation, and reports content-free counters; `verify_plan` re-derives uniqueness, name validity, budgets, and recorded lengths from scratch.
- Both carry `#![allow(dead_code)]`. The plan was to remove it when the writer landed (M4c). It is still there; trying its removal (and fixing whatever clippy then reports) is an open housekeeping item.

Not yet implemented, reported instead of hidden: flattening of over-budget folder chains (rule L4 step 4) and the identity-name fallback (step 5). When even the floor names cannot fit, `budget_exceeded` counts the entries.

Gate (met on Windows):
- Unit tests for every rule, including astral-plane characters counted as two units, `NUL` as a directory name, trailing dots and spaces, NFC versus NFD, case-only differences, empty names, `folder.json` collisions, attachment-file versus embedded-directory collisions, and suffix widths for groups of 2, 9, 10, 99, 100, 101, 999, 1,000, and 1,500.
- Property tests (`proptest`): sanitizing is idempotent and always yields a valid component; shortening never exceeds the limit; assignment is unique, deterministic, and independent of input order; plans of arbitrary two-level trees pass every gate and ignore source order.
- The build, `clippy -D warnings`, and the full test suite (66 existing tests plus the new ones) pass.

### M4b-3 — naming census and folder-identity spike (counts only)

Status: **built and verified on Windows (v0.1.22, 2026-10-02), with open items below.** Build clean, 117 tests passing. New modules `src/dry_run.rs`, `src/source_msg.rs`, `src/source_pst.rs`; `src/cli.rs` gains `--out <DIR>` and `--dry-run`; `src/main.rs` dispatches to the dry run. The owner also fixed `root_stem` for Windows paths. Full results: [`../verification/m4-results.md`](../verification/m4-results.md). The census was re-run on v0.1.23.1 and is unchanged.

`tsp <input> --out <dir> --dry-run` reads a PST, a `.msg` file, or a directory of `.msg` files (planned recursively, mirroring subdirectories), plans the export, and prints only counts in the stable `key=value` vocabulary: `plan_*` keys from the planner, `plan_gate_*` keys and `plan_gate_violations` from the independent re-check, `source_*` keys for what the source contained, and `folder_identity_*` keys for the spike. It writes nothing. (At v0.1.22 `--out` without `--dry-run` stopped with a message; since M4c it exports.)

Folder-identity spike: `outlook-pst` v1.2.0 exposes, for every folder, its node ID (`FolderProperties::node_id()`, unique within the PST), its display name, and a computed `PidTagEntryId` (property 0x0FFF, built from the store UID and the node ID). It exposes no separate record-key property for folders. **Answered from the run:** on `tsp-tester.pst`, a node ID and an entry ID were both available for all 11 folders walked and none were unavailable (`folder_identity_nid_available=11`, `folder_identity_entry_id_available=11`, `folder_identity_unavailable=0`).

Subject marker: `PidTagSubject` may begin with U+0001 followed by one more character recording the prefix length (the crate's own examples strip it). Both builders remove those two characters and count them (`source_subject_markers_stripped`; 60 on the PST fixture, 0 in the `.msg` directory).

Limits reported, not hidden: PST attachment names come from the attachment table, which may carry only the short (8.3) name (`source_attachment_tables_with_long_name_column` says how often a long-name column exists; 9 on the PST fixture), and PST embedded-message attachments are planned as plain files and counted (`source_embedded_attachments_not_opened`; 3).

Gate, against the run:
- **`plan_gate_violations=0` and `plan_budget_exceeded=0` on both inputs: met for the `.msg` directory; not met for the PST at the long output path used** (4 over-budget entries at a root of 152 units, longest path 266 against 259). This is the budget gate working as designed under a deliberately long `--out`; it still has to be shown at a short `--out`, which has not been run.
- **Predicted counts: not matched.** Predicted PST `plan_folders=9`, `plan_messages=57`, `plan_attachment_files=79`; census 10 / 60 / 84. Predicted `.msg` directory 29 / 1 / 29; census 34 / 3 / 34, probably because the recursive scan includes 3 subdirectories (not itemized). The PST difference is unreconciled; it needs the current PST diagnostic output.
- The counts are reviewed with the owner, and the fixtures received so far are added: the counts were reviewed on 2026-10-03; whether the fixture set changed since the earlier diagnostic is not established.
- The folder-identity question is answered from the run (above).
- Output of the existing modes is unchanged: met (`verify-split.ps1`: default, `--verify`, and PST outputs IDENTICAL to v0.1.19; repeated on v0.1.23.1).

### M4a-2 — accept the new ADRs

Using the census, accept up to five ADRs that **complete** the archive ADR: (1) export naming, collisions, and path budgets; (2) message and folder identity and provenance; (3) output posture (`--out`, `--dry-run`, consent, ownership by `folder.json`); (4) body and formatting-loss policy; (5) `metadata.json` / `folder.json` schemas and per-item status. The archive ADR gets a cross-reference in "More Information" (status and layout unchanged).

Status: not started. M4c now supplies what ADR 3 and ADR 5 need to be concrete (the consent rules and the draft schemas).

Gate: ADRs accepted by the owner; a hand-written example archive for one corpus message (private content replaced) that follows them and passes the plan's own invariants. The archive that M4c writes can serve as the starting point.

### M4b-4 — normalized model types

`OutlookItem` and parts: provenance, properties (typed, raw bag preserved), recipients, bodies (each variant with raw bytes and detected encoding), attachments (data source and status), embedded items, named properties, diagnostics. Pure data.

Status: not started. `model.rs` holds only the seed M4c needs (`MessageContent`: subject, Internet message ID, time, plain-text body, which other body forms exist).

Gate: invariants tested; a table mapping each existing counter category to its model field.

### M4c — walking skeleton

Status: **built and passing its synthetic tests (v0.1.23, tag v0.1.23.1, 2026-10-03, 155 tests); real-corpus gate item open.** New modules `model.rs`, `archive.rs` (pure rendering), `export.rs` (preflight, consent, staged write, read-back); `source_msg.rs` gains the message-to-file map and `read_message_content`; `cli.rs` gains `--overwrite` and `--no-source-hash`; `root_stem` ignores trailing separators. New dependencies: `serde`, `serde_json`, `sha2`.

What it does: for `.msg` input (a file, or a directory handled recursively), plan names with the M4b planner and refuse a plan that breaks a gate; render the archive in memory (`message.md`, `metadata.json`, `folder.json`, root `folder.json`); compare it with the target (counts only); decide consent; write each file through `<out>/.tsp-tmp/<n>` and rename it into place; read everything back; print `export_*` counts and one `export_tree_sha256`. Attachments and embedded messages are counted and recorded as not extracted, and no `attachments/` directory is written. `.pst` input stops with a "not implemented" error (M4i).

Decisions taken in M4c (the owner is asked to confirm them, since the ADRs have not been written):
- **Consent.** Replacing a file the tool generated earlier needs `--overwrite` or an interactive yes; non-interactive without it, the run is refused (exit code 2). A target directory the tool did not create is refused even with `--overwrite`. An archive recorded for a different source (different kind or name) is refused even with `--overwrite`; the source hash is deliberately not part of source identity, so re-exporting a changed source is allowed with consent.
- **Interrupted runs.** The root `folder.json` is written first as `incomplete` and replaced with the `complete` version last. Whole-archive atomicity is not provided; per-file replacement is atomic.
- **Staging.** A short numeric name under `.tsp-tmp` (as planned for M4h), with the `--out` path limited so staged paths cannot exceed final ones. Leftover numeric files from an interrupted run are removed; anything else in `.tsp-tmp` is an error.
- **Body.** Provisional: the subject as a heading, the plain-text body verbatim in a code fence (longer than any backtick run in the body). The body policy ADR (M4e) decides the final form.
- **Status.** Every M4c message is `partial`, with reasons (envelope not extracted; attachments not extracted; formatted bodies not converted).
- **Housekeeping not done:** `#![allow(dead_code)]` in `naming.rs` and `plan.rs`; the `Cargo.toml` version is `0.1.23` while the tag is `v0.1.23.1`.

Gate:
- Golden files committed for synthetic fixtures (no personal data), run in CI on Linux and Windows: **committed and passing locally on Windows; the CI result has not been reported.**
- Two exports of the same input are byte-identical (tree hash): **met by a synthetic test; not yet shown on corpus messages.**
- Overwrite matrix tested (empty target, identical existing files, different existing files, unrelated files, non-interactive without consent, foreign directory, other source, over-budget plan): **met by synthetic tests.**
- One real corpus message exported and reviewed by the owner against a checklist: **not done. This is the remaining M4c gate item.**

### M4d — envelope: headers, recipients, properties

Subject, sender, recipients, dates, importance, Internet message ID, transport headers, conversation fields. Candidate property IDs are from memory and each must be confirmed against MS-OXPROPS first.

Gate: differential check against `msg_parser` for the fields it exposes, counts only, every mismatch triaged against the specifications; unit tests and inspection for the rest.

### M4e-0 — HTML-to-Markdown bake-off (approved)

Follow [`m4a-dependency-research.md`](m4a-dependency-research.md): candidates `htmd`, `html2markdown`, `html-to-markdown-rs`, behind an `HtmlToMarkdown` trait, judged on determinism, panics, content preservation, tables, links and images, licensing, dependency weight, and custom-handler support.

Gate: a recorded choice with the exact version pinned, in the body-policy ADR.

### M4e-1 — MS-OXRTFEX de-encapsulation (in-house)

Implement the module specified in the research document, including confirming the recognition rule (first 10 tokens) against `check_compressed_rtf_bytes`.

Gate: spec-derived unit and golden tests; a weak-oracle differential against `msg_parser`'s `html_from_rtf()` on the 27 encapsulated-HTML corpus messages, counts only, every disagreement triaged; a property test that arbitrary input never panics and stays within an output bound.

### M4e-2 — body pipeline

Body selection (native HTML, HTML from RTF, RTF, plain text) per the ADR; Outlook/Word preprocessing and `cid:` mapping to files in `attachments/`; conversion via the chosen crate into `message.md`; the original body kept verbatim inside the message folder (file names and placement decided in the body-policy ADR); formatting loss recorded in `metadata.json`.

Gate: golden tests on synthetic bodies for each variant and tricky case; a corpus run reporting per-variant conversion counts and failures; a content-preservation check (all visible text present, order preserved); owner review of a sample.

### M4f — attachments

Write by-value attachment bytes into the message's `attachments/`; sanitize and de-duplicate names (N and U rules); record size and SHA-256; zero-byte files written empty and flagged; inline attachments marked `inline: true`; embedded messages written as nested message folders inside `attachments/` under the depth cap (default 3); OLE and by-reference recorded as not extracted with reasons.

Gate:
- Byte-level differential: SHA-256 of each extracted by-value attachment equals the hash of `msg_parser`'s payload, counts only.
- Adversarial synthetic fixtures (T6): reserved names, trailing dots and spaces, 255+ names, duplicates, case-only differences, empty and missing names, Unicode names, control characters, deep embedded chains.
- Every corpus attachment appears in the archive or in diagnostics, and the two totals equal `total_attachments`.
- Path budget respected for the deepest attachment of every message.

### M4g — metadata, diagnostics, per-item status

Implement the `metadata.json` and `folder.json` schemas from the ADR: identity and provenance, property bag, recipients, attachment records, named properties, extraction diagnostics, per-item status (complete, partial, failed, each with a closed list of reasons), applied renames and truncations, and the folder's child index, and the content SHA-256 with `identical_to` (Q11, the only open definition). Stdout summary of counts.

Gate: JSON schema tests on synthetic archives; the corpus stdout summary reconciles with `--verify` totals (every message accounted for); no absolute paths and no export-time values in any metadata file.

### M4h — writer hardening

Whole-archive atomicity or an equally clear recovery story (M4c writes per file and marks the root `incomplete` until the end); streaming instead of rendering the whole archive in memory; replace-on-rename behavior verified on Windows; deterministic ordering and line endings (LF, UTF-8 without BOM); modification-time policy; partial-failure behavior (one failed message never corrupts or hides others); FAT32 directory-size warning; `--long-paths` and `--max-relative-path` options.

Gate: run-twice byte-identical trees on the corpus (single tree-hash line); fault injection (unwritable target, simulated disk full where practical, corrupt input mid-batch); results independent of input order; consent flow re-tested.

### M4i — PST into the model

Known constraint P4c: attachment bytes and embedded-message content are unreachable through `outlook-pst` v1.2.0's public API. Options: (1) accept and flag: archive attachment metadata, mark bytes not extracted (recommended first); (2) go below the public API; (3) raise the gap upstream (recommended in parallel).

Work: PST messages through the same body pipeline; folder hierarchy into the layout; folder identity per the spike (both identifiers were available on the fixture); ANSI PST support if `outlook-pst` provides it.

Gate: the diagnostic's counts reconcile with the archive (every message and attachment accounted for as extracted or flagged; the expected totals depend on resolving the open count difference recorded in M4b-3); plan counters match census counters; a `libpff` or `libpst` differential is scoped even if executed later.

### M4j — end-to-end verification and close

`--verify`-style archive read-back (counts only): messages, attachments by status, body variants, hash matches, schema violations, entries not listed in their folder's `folder.json`; the plan re-derived from the archive and compared with the recorded plan. Corpus-gated regression test in the `TSP_FIXTURE_DIR` pattern. Manual review protocol for private content. Documentation and ADR confirmations updated.

Gate: all earlier gates re-run on the final build; every coverage-matrix row for M4 capabilities verified or marked with its limit; the exit criteria below.

## Exit criteria

1. Exporting the 29-message corpus and the fixture PSTs produces archives in which every message and attachment is accounted for as complete, partial (with reason), or failed (with reason).
2. Two runs over the same input give byte-identical archives.
3. Extracted attachment bytes match the oracle's for every by-value attachment `msg_parser` can read.
4. No path exceeds the budget; every namespace is collision-free under the case-insensitive key; asserted by the plan gates and by read-back.
5. Stdout is content-free; the archive contains no absolute paths or export-time values; no existing file is replaced without consent.
6. Synthetic golden and adversarial tests run in CI; corpus-dependent checks are gated and documented.
7. Documentation and ADRs match the code, with verified and unverified items stated separately.

## Fixtures (owner is creating these separately)

Results reach me as the output of your runs (`tsp <pst>`, then the census), plus a short note on how each was made; the PST files do not need to be sent. Priority is by risk removed.

| Priority | Fixture | Why | How to create |
|---|---|---|---|
| 1 | **ANSI PST** (Outlook 97-2002 format), small, mixed content, including non-Latin text in a Windows code page | Only way to exercise `PT_STRING8` decoding on real data and ANSI-store handling in `outlook-pst` | Outlook data file dialog, 97-2002 format if still offered; otherwise an old archive |
| 2 | **Adversarial-names PST** | Exercises N, L, U rules on real Outlook data | Subjects with `< > : " / \ \| ? *`, `CON`, `NUL`, trailing dots and spaces, an empty subject, 255- and 300-character subjects, emoji, right-to-left text, combining marks, an NFC and an NFD form of the same word; folders with the same problems (the UI blocks some characters, so use automation or another tool); sibling folders differing only by case |
| 3 | **Duplicates PST** | Tests U rules: position suffixes, ordering by time, large groups | 6+ different messages with one subject in one folder; a message copied within a folder and across folders; a conversation with 10 `RE:` replies; one folder with 100+ messages sharing a subject (or `(no subject)`) to cross the two-digit boundary; same-name sibling folders if any tool can produce them |
| 4 | **Deep and wide PST** | Path budget, flattening, directory size | 25-30 nested levels with moderately long names; one folder with several thousand small messages |
| 5 | **Attachments PST** | M4f | Duplicate attachment names in a message; an attachment with no name; a 200-character attachment name; several inline images; an embedded message three levels deep; an OLE object; a zero-byte attachment; one 50-200 MB attachment; `desktop.ini`; a reserved device name |
| 6 | **Non-mail items PST** | Scope (rules section 12) | A calendar entry, contact, task, note, meeting request, and a bounce (NDR) |
| 7 | **Other producers** | The corpus is one producer | PSTs from an Exchange export, a third-party migration tool, and a current Outlook build |
| 8 | **Encrypted or password PSTs** | `outlook-pst` may not read them | One with compressible encryption, one with a password |
| 9 | **Damaged PST** | Adversarial (T6) | A copy of a small PST truncated partway |
| 10 | **More `.msg` files** | Bodies (M4e) | Word-authored HTML with tables and colors; signature images; a genuine RTF-only message; ANSI-encoded messages; messages from other clients saved as `.msg` |

Privacy: any of these can be synthetic, and census outputs are counts, so they are safe to paste. Do not paste subjects or names.

## Architecture note: separate crates (not a goal now)

The module split exists for navigability and so that new pure modules (`naming`, `plan`, the model) can be developed and tested without the adapters. It is not a first step toward a Cargo workspace. All items are `pub(crate)` because the split was mechanical, so the current boundaries are not a designed public API, and the crate is `publish = false`.

Crate boundaries become worthwhile only for a concrete reason: another program embedding the engine, enforcing that the model never depends on an adapter, isolating heavy dependencies (`outlook-pst`, `msg_parser`, an HTML converter) so they compile and change separately, or independent versioning. None applies today. Decision: stay a single binary crate through M4, keep `naming`, `plan`, and the model free of imports from `pst`, `oxmsg_*`, and `verify` (a test can check the `use crate::` lines), and revisit at M4j with evidence such as build times and the shape of the interfaces. If the boundary is ever drawn, a plausible set is: model, naming, and plan; the MSG adapter; the PST adapter; the archive writer; and the CLI with `--verify`.

## Principal risks

- **Naming policy** (M4a/M4b). Naming decides re-export stability; a wrong choice forces re-exporting archives. Mitigation: pure engine, census before the ADRs are accepted, property tests.
- **Path budget in a directory-per-message layout.** Every message is a directory, so the 248 rule and worst-case child reservation bind tightly; long subjects plus long attachment names will be truncated often on long roots. The PST fixture already shows entries that cannot fit at a root of 152 units (4 of 154). Mitigation: budget from the real root, report truncation counts, `--max-relative-path`, guidance to export near a drive root. M4c refuses to export a plan that breaks a gate; whether a better answer exists (flattening, L4 step 4) is open.
- **Bodies** (M4e). De-encapsulation and HTML-to-Markdown quality on Word HTML; partly subjective. Mitigation: bake-off, content-preservation check, verbatim body kept.
- **PST attachments** (M4i). Blocked by the public API.
- **PST counts unreconciled.** The planner and the earlier PST diagnostic disagree on the fixture's size (see M4b-3); until resolved, the M4i reconciliation gate has no trusted baseline.
- **Folder identity.** Both identifiers were readable on the fixture; the NID-derived identifier is still unproven across other producers. The fallback ordering is deterministic but weaker.
- **Content-hash definition** (Q11) now affects only the `identical_to` metadata, not any name, so a poor first definition is cheap to correct.
- **Very large duplicate groups.** Thousands of same-subject messages in one folder widen their suffix and rename the group once when it crosses 99 or 999 members. Mitigation: per-group width, census counts of large groups (the fixture's largest group is 16).
- **Schemas frozen too early or too late.** M4c produces draft schemas ahead of the ADRs on purpose; the risk is treating them as final. They carry `schema_version` `0.1-draft`.
- **Export untested on real messages.** M4c is verified by synthetic tests only; real corpus messages may expose cases the synthetic ones do not (large bodies, unusual encodings, long subjects).
- **Memory use.** The writer renders the whole archive in memory first; a very large mailbox will need the streaming work in M4h.
- **Uninformative file names.** Every message is called `message.md`, so search results and editor tabs show the folder only as context. Accepted with the ADR; `message.md` can open with a title line (body-policy ADR), and M4c's draft does.
- **Single-producer corpus.** Addressed by the fixture table.
- **Windows-only behavior** is verifiable only on Windows; CI covers unit tests on Linux and Windows.
- **Dependencies.** Each new crate is a maintenance risk; pin, read source, check license.

## Decisions still needed from the owner

1. A real-corpus export run and review (the remaining M4c gate item), a dry run with a short `--out`, and the current PST diagnostic output to reconcile the PST counts.
2. Whether M4c's consent rules, staging, and draft schemas are acceptable as built (the ADRs, M4a-2, follow).
3. Whether to try removing `#![allow(dead_code)]` now, and how to reconcile the `0.1.23` Cargo version with the `v0.1.23.1` tag.
4. Q11, the content-hash definition, before M4g (not before).
