# M4 plan (v2) — normalized model and deterministic Markdown archive

Status: **proposed, not started.** Version 2, revised 2026-09-30 after the project owner's answers to the first plan's open questions and after review of external research on PST-to-Windows-filesystem export. Nothing here has been implemented; every stage has an evidence gate that must be met on Windows before the stage counts as done. Companion documents:

- [`m4a-export-rules.md`](m4a-export-rules.md): the draft naming, layout, duplicate, path-length, and overwrite rules, with the open questions Q1-Q8.
- [`m4a-dependency-research.md`](m4a-dependency-research.md): RTF de-encapsulation, HTML-to-Markdown converters, and supporting crates.

## What changed from v1

1. **Decisions recorded from the owner (2026-09-30):**
   - Staged order accepted: module split first, walking skeleton before breadth.
   - Export mirrors the PST folder tree; message files are named from subjects; duplicates use Windows-style suffixes; path length is handled up front; `tsp` warns and asks before overwriting.
   - Same-name sibling folders are told apart by the folders' own identifiers.
   - A message with attachments gets a `<subject> - attachments` folder.
   - `serde`, `serde_json`, and a hash crate approved.
   - More `.pst` fixtures are available on request.
2. **Layout changed.** The owner's layout (`<subject>.md` plus `<subject> - attachments/`) differs from the layout the accepted ADR "Deterministic Markdown archive" chose (one folder per message with `message.md`, `metadata.json`, `attachments/`). A superseding ADR is required; the determinism and Markdown-as-projection decisions of that ADR are kept.
3. **New early work: a pure naming/plan engine and a counts-only "naming census".** From the external research: the mapping rules are deterministic and can be a pure function, testable with property tests and reusable as a `--dry-run`. The census turns the identity and layout decisions from guesses into evidence before the ADRs are frozen. The research's scanner is adopted as `--dry-run` inside the export mode, not as a separate tool.
4. **M4a splits in two.** Draft now (M4a-1); freeze after the census (M4a-2).
5. **Research findings folded in:** no Rust crate performs MS-OXRTFEX de-encapsulation (write it in-house); HTML-to-Markdown gets a bake-off with a provisional preference for `htmd`; SHA-256 via `sha2`.
6. **What the research contributed and what was not adopted.** Adopted: the Windows and PST rule sets, the mapping options (sanitize with deterministic suffix; budget-aware flattening; attachments beside the message), the plan/dry-run split, property testing of the naming engine, and the check catalog (as plan counters). Not adopted: date-prefixed, hash-suffixed filenames (they conflict with the owner's requirement that filenames mirror subjects); `libpff` FFI (unnecessary; `outlook-pst` is pure Rust); `serde_yaml` (reportedly archived); the `windows` crate and `rayon` (unneeded now). Two of its claims are treated as unverified: that EntryIDs "survive as long as the object is not modified", and that FAT variants have a 260-character path limit "in practice" (260 is a Win32 API limit, not a FAT limit).

## Goal

Turn what the two adapters read into a durable archive: a Windows-safe directory tree mirroring the source's folders, one `.md` per message named from its subject, attachments in a sibling folder, and tool-owned metadata that carries identity, provenance, and loss status. M4 is the first milestone where `tsp` writes message content to disk; stdout stays counts-only.

## Where M3 leaves us

- The MSG adapter decodes typed properties, value streams, named properties, recipients, attachments (methods, content IDs, emptiness), and one level of embedded message. It reads content into memory and prints only counts.
- The PST adapter is a counter-only diagnostic over `outlook-pst` v1.2.0. It cannot open embedded-message or OLE attachments (P4c).
- `msg_parser` is an oracle for comparable MSG fields; no PST oracle has been run.
- No normalized model, body extraction for output, renderer, or writer exists. All code is one 5,070-line `main.rs`.
- Fixtures: 29 `.msg` files (27 carry HTML only inside RTF; 23 attachments carry a content ID; 1 embedded message; 1 zero-byte attachment; no `PT_STRING8` value) and one enhanced PST (57 messages, 79 attachments).

## Working rules (unchanged)

- Staged, each stage with an evidence gate; done means met on Windows.
- Claude drafts uncompiled; the owner runs the quality gate and supplies output. Docs are updated per stage and separate verified from written.
- Stdout is content-free at every stage. Archive content goes only to the directory the user names.
- Loss is explicit: never silently dropped.
- Symmetry between adapters where the PST side allows.
- Sources of truth: Microsoft's specifications for formats and Win32 naming; independent implementations as oracles, not authorities.

## Stages

### M4a-1 — draft decisions and rules (documentation)

Done with this version: [`m4a-export-rules.md`](m4a-export-rules.md) and [`m4a-dependency-research.md`](m4a-dependency-research.md).

Gate: the owner answers Q1-Q8 in the rules document and the decisions below.

### M4b-1 — mechanical module split

Move `main.rs` into modules along the seams the README documents: CLI, shared vocabulary, PST diagnostic, MSG report, custom parser (structure, decoding, extraction), verification.

Gate: default `.msg` output, `--verify` output, and PST output are byte-identical before and after (saved reports diffed); build, clippy, and the test count unchanged (66).

### M4b-2 — pure naming and planning modules

New modules with no I/O and no PST or MSG types:

- `naming`: sanitize (N-rules), UTF-16 length measurement, shortening (L4/L5), collision keys (U1), suffix assignment (U2-U8).
- `plan`: given a tree of names, timestamps, identifiers, content hashes, and a policy (budget, root length), produce the full list of final paths with flags and counters.

Gate:
- Unit tests for every rule, including astral-plane characters counted as two units, `CON`/`NUL.md`, trailing dots and spaces, NFC versus NFD names, case-only differences, empty names, `.tsp` at the root, and file-versus-folder collisions.
- **Property tests** (`proptest` as a dev-dependency, if approved) over arbitrary names and trees: sanitizing is idempotent; no final component contains a reserved character; every path is within budget; no two entries in a directory share a collision key; output is identical across runs and independent of input order.
- Coverage of each research check (C-01 to C-18 as applicable) as a named plan counter.

### M4b-3 — naming census and folder-identity spike (counts only)

Add `tsp <input> --out <dir> --dry-run`: runs the planning phase over a PST or `.msg` input and prints only counts, in the stable `key=value` vocabulary and with a `plan_gate_violations` total in the `--verify` style. It writes nothing.

Keys (proposed): names needing sanitization by class; reserved-name hits; trailing-dot/space hits; empty subjects; components truncated; maximum component and path lengths in UTF-16 units; maximum folder depth; folders flattened; collision groups (messages, folders, attachments); content-identical duplicate groups; messages missing `PidTagInternetMessageId`; duplicate `PidTagInternetMessageId`; attachments without names; attachment name collisions; budget exceeded; maximum entries per directory.

Spike: determine what `outlook-pst` v1.2.0 exposes as a folder identifier (NID, `PidTagRecordKey`, EntryID) and record it.

Gate: the census runs on the 29-message `.msg` set, the 57-message PST, and every fixture from the fixture table below; the counts are reviewed; the folder-identity question is answered (available, not available, or partly).

### M4a-2 — freeze the ADRs

Using the census, accept up to five ADRs: export layout and naming (superseding the layout part of "Deterministic Markdown archive"); message and folder identity and duplicate policy; output posture (`--out`, `--dry-run`, consent, `.tsp` ownership); body and formatting-loss policy; metadata and per-item status schema.

Gate: ADRs accepted by the owner; a hand-written example archive for one corpus message (private content replaced) that follows them; the example passes the plan's own invariants.

### M4b-4 — normalized model types

`OutlookItem` and parts: provenance, properties (typed, raw bag preserved), recipients, bodies (each variant with raw bytes and detected encoding), attachments (data source and status), embedded items, named properties, diagnostics. Pure data.

Gate: invariants tested; a table mapping each existing counter category to its model field.

### M4c — walking skeleton on synthetic input

Thin path end to end for the simplest case: a synthetic plain-text `.msg` (built at test time, as the ANSI tests do) with no attachments through model, plan, consent, staged write, and read-back.

Includes the overwrite behavior: counts-only preflight, interactive prompt, non-interactive refusal without `--overwrite`, and "never touch files we did not create".

Gate:
- Golden files committed for synthetic fixtures (no personal data), run in CI on Linux and Windows.
- Two exports of the same input are byte-identical (tree hash).
- Overwrite matrix tested: empty target, existing identical files, existing different files, unrelated files, non-interactive without consent.
- One real corpus message exported and reviewed by the owner against a checklist.

### M4d — envelope: headers, recipients, properties

Subject, sender, recipients, dates, importance, Internet message ID, transport headers, conversation fields. Candidate property IDs are from memory and each must be confirmed against MS-OXPROPS first.

Gate: differential check against `msg_parser` for the fields it exposes, counts only, every mismatch triaged against the specifications; unit tests and inspection for the rest.

### M4e-0 — HTML-to-Markdown bake-off

Follow the process in [`m4a-dependency-research.md`](m4a-dependency-research.md): candidates `htmd`, `html2markdown`, `html-to-markdown-rs`, behind an `HtmlToMarkdown` trait, judged on determinism, panics, content preservation, tables, links/images, licensing, dependency weight, and custom-handler support.

Gate: a recorded choice with exact version pinned, in the body-policy ADR.

### M4e-1 — MS-OXRTFEX de-encapsulation (in-house)

Implement the module specified in the research document, including confirming the recognition rule (first 10 tokens) against `check_compressed_rtf_bytes`.

Gate: spec-derived unit and golden tests; differential (weak oracle) against `msg_parser`'s `html_from_rtf()` on the 27 encapsulated-HTML corpus messages, counts only, every disagreement triaged; property test that arbitrary input never panics and stays within an output bound.

### M4e-2 — body pipeline

Body selection (native HTML, HTML from RTF, RTF, plain text) per the ADR; Outlook/Word preprocessing and `cid:` mapping; conversion via the chosen crate; original body kept verbatim in the tool-owned store; formatting loss recorded (color, highlight, font, layout, embedded objects).

Gate: golden tests on synthetic bodies for each variant and tricky case; a corpus run reporting per-variant conversion counts and failures; a content-preservation check (all visible text present, order preserved); owner review of a sample.

### M4f — attachments

Write by-value attachment bytes into `<final stem> - attachments/`; sanitize and de-duplicate names (N/U rules); record size and SHA-256; zero-byte files written empty and flagged; inline images linked from Markdown by `cid:` mapping; embedded messages written as nested messages under the depth cap; OLE and by-reference recorded as not extracted with reasons.

Gate:
- Byte-level differential: SHA-256 of each extracted by-value attachment equals the hash of `msg_parser`'s payload, counts only.
- Adversarial synthetic fixtures (T6): reserved names, trailing dots/spaces, 255+ names, duplicates, case-only differences, empty and missing names, Unicode names, control characters, deep embedded chains.
- Every corpus attachment appears in the archive or in diagnostics, and the two totals equal `total_attachments`.
- Path budget respected for the deepest attachment of every message.

### M4g — metadata, diagnostics, per-item status

Tool-owned `.tsp/` store: `manifest.json` (every folder and message, identity, original names and paths, applied renames, budget used, hashes) and per-message JSON (property bag, recipients, attachment records, status). Status vocabulary: complete, partial, failed, each with a closed list of reasons. Content hash for messages (needed by rule U4). Stdout summary of counts.

Gate: JSON schema tests on synthetic archives; corpus stdout summary reconciles with `--verify` totals (every message accounted for); no absolute paths and no export-time values in metadata.

### M4h — writer hardening

Staged and atomic writes; short fixed staging path; replace-on-rename behavior verified on Windows; deterministic ordering and line endings (LF, UTF-8 without BOM); modification-time policy; partial-failure behavior (one failed message never corrupts or hides others); FAT32 directory-size warning.

Gate: run-twice byte-identical trees on the corpus (single tree-hash line); fault injection (unwritable target, simulated disk full where practical, corrupt input mid-batch); results independent of input order; consent flow re-tested.

### M4i — PST into the model

Known constraint P4c: attachment bytes and embedded-message content are unreachable through `outlook-pst` v1.2.0's public API. Options: (1) accept and flag: archive attachment metadata, mark bytes not extracted (recommended first); (2) go below the public API; (3) raise the gap upstream (recommended in parallel).

Work: PST messages through the same body pipeline; folder hierarchy into the layout; folder identity per the spike; ANSI PST support if `outlook-pst` provides it.

Gate: the diagnostic's counts reconcile with the archive (57 messages and 79 attachments accounted for as extracted or flagged); plan counters match census counters; a `libpff` or `libpst` differential is scoped even if executed later.

### M4j — end-to-end verification and close

`--verify`-style archive read-back (counts only): messages, attachments by status, body variants, hash matches, schema violations, files not in the manifest; the plan re-derived from the archive and compared with the recorded plan. Corpus-gated regression test in the `TSP_FIXTURE_DIR` pattern. Manual review protocol for private content. Documentation and ADR confirmations updated.

Gate: all earlier gates re-run on the final build; every coverage-matrix row for M4 capabilities verified or marked with its limit; the exit criteria below.

## Exit criteria

1. Exporting the 29-message corpus and the fixture PSTs produces archives in which every message and attachment is accounted for as complete, partial (with reason), or failed (with reason).
2. Two runs over the same input give byte-identical archives.
3. Extracted attachment bytes match the oracle's for every by-value attachment `msg_parser` can read.
4. No path exceeds the budget; every directory is collision-free under the case-insensitive key; asserted by the plan gates and by read-back.
5. Stdout is content-free; the archive contains no absolute paths or export-time values; no existing file is replaced without consent.
6. Synthetic golden and adversarial tests run in CI; corpus-dependent checks are gated and documented.
7. Documentation and ADRs match the code, with verified and unverified items stated separately.

## Fixtures requested (owner offered more PSTs; this is the optimal set)

I cannot read PSTs or run Rust here, so the value of a fixture reaches me as the output of your runs (`tsp <pst>`, then the census output), plus a short note on how it was created. The PST files themselves do not need to be sent. Priority is by how much risk each removes.

| Priority | Fixture | Why | How to create |
|---|---|---|---|
| 1 | **ANSI PST** (Outlook 97-2002 format), small, mixed content, including non-Latin text in a Windows code page | Only way to exercise `PT_STRING8` decoding on real data and ANSI-store handling in `outlook-pst` | Outlook, Data File dialog, choose the 97-2002 format if still offered; otherwise a PST from an old archive |
| 2 | **Adversarial-names PST** | Exercises N/L/U rules on real Outlook data | Subjects containing `< > : " / \ | ? *`, `CON`, `NUL`, trailing dots and spaces, an empty subject, 255-character and 300-character subjects, emoji, right-to-left text, combining marks, an NFC and an NFD version of the same word; folders with the same problems (the UI blocks some characters, so use automation or another tool for those); sibling folders differing only by case |
| 3 | **Duplicates PST** | Tests U-rules and the content-identical versus same-subject distinction | 6+ different messages with the same subject in one folder; a message copied within a folder and across folders; a conversation with 10 `RE:` replies; same-name sibling folders if any tool can produce them (an import may) |
| 4 | **Deep and wide PST** | Path budget, flattening, directory size | 25-30 nested levels with moderately long names; one folder with several thousand small messages |
| 5 | **Attachments PST** | M4f | Duplicate attachment names in one message; attachment with no name; a 200-character attachment name; several inline images (signature logos and pasted pictures); an embedded message three levels deep; an OLE object; a zero-byte attachment; one 50-200 MB attachment; an attachment named `desktop.ini`; an attachment with a reserved device name |
| 6 | **Non-mail items PST** | Scope decision (section 12 of the rules) | A calendar entry, contact, task, note, meeting request, and a bounce (NDR) |
| 7 | **Other producers** | The corpus is one producer | A PST from Exchange export; one from a third-party migration tool; one from a current Outlook build versus the existing one |
| 8 | **Encrypted or password PSTs** | `outlook-pst` may not read them | One with "compressible encryption", one with a password (the password is only a CRC gate per the research, but the file mode differs) |
| 9 | **Damaged PST** | Adversarial (T6) | A copy of a small PST truncated partway |
| 10 | **More `.msg` files** | Bodies (M4e) | Word-authored HTML with tables and colors; signature images; a genuine RTF-only message; ANSI-encoded messages; messages from other clients saved as `.msg` |

Privacy: any of these can be synthetic. The census outputs are counts, so they are safe to paste. Do not paste subjects or names.

## Principal risks

- **Filename policy** (M4a). Naming decides re-export stability; a wrong choice forces re-exporting archives. Mitigation: pure engine, census before freeze, property tests.
- **Bodies** (M4e). De-encapsulation and HTML-to-Markdown quality on Word HTML; partly subjective. Mitigation: bake-off, content-preservation check, verbatim body kept.
- **PST attachments** (M4i). Blocked by the public API.
- **Folder identity.** The NID-derived identifier may not be exposed by `outlook-pst`; the fallback ordering is deterministic but weaker.
- **Single-producer corpus.** Addressed by the fixture table.
- **Windows-only behavior** is verifiable only on Windows; CI covers unit tests on Linux and Windows.
- **Superseding an accepted ADR** (layout). Must be done explicitly, not by drift.
- **Dependencies.** Each new crate is a maintenance risk; pin, read source, check license.

## Decisions needed from the owner

1. Answer Q1-Q8 in `m4a-export-rules.md` (root naming; suffix scheme; where metadata lives; replacement characters; scope of non-mail items; inline attachments; long paths and default budget; folder markers).
2. Approve `proptest` as a dev-dependency, and `unicode-normalization` and `sha2` as dependencies (A6 covered `serde`, `serde_json`, and "a hash crate").
3. Approve the M4e-0 bake-off and, unless it fails, the provisional choice of `htmd`.
4. Confirm superseding the per-message-folder layout in the accepted archive ADR.
5. Supply fixtures in the priority order above, at least priorities 1-3 before M4b-3.
