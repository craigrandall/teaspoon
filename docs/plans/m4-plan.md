# M4 plan (v4) — normalized model and deterministic Markdown archive

Status: **M4a-1 decided; M4b-1 drafted and awaiting its first build.** Version 4, revised 2026-09-30 after the project owner's decisions on the remaining open questions (Q7-Q11). Nothing here has been implemented; every stage has an evidence gate that must be met on Windows before the stage counts as done. Companion documents:

- [`m4a-export-rules.md`](m4a-export-rules.md): the draft naming, layout, identity, duplicate, path-length, and overwrite rules (v3, aligned with the accepted archive ADR).
- [`m4a-dependency-research.md`](m4a-dependency-research.md): RTF de-encapsulation, HTML-to-Markdown converters, and supporting crates.

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

Done with this version: [`m4a-export-rules.md`](m4a-export-rules.md) v3 and [`m4a-dependency-research.md`](m4a-dependency-research.md). Every question in the rules document is decided except Q11 (the exact content-hash definition), which is metadata-only and waits for M4g.

### M4b-1 — mechanical module split

Status: **drafted (uncompiled), awaiting the owner's first build.** The approach and the compile risks are described with the delivered files (`src-split/`).

Move `main.rs` into modules along the seams the README documents: CLI, shared vocabulary, PST diagnostic, MSG report, custom parser (structure, decoding, extraction), verification. No behavior change.

Gate: default `.msg` output, `--verify` output, and PST output byte-identical before and after (saved reports diffed); `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and the same 66 tests passing.

### M4b-2 — pure naming and planning modules

New modules with no I/O and no PST or MSG types:

- `naming`: sanitize (N-rules), UTF-16 length measurement, shortening (L4/L5), collision keys (U1), suffix assignment (U2-U8; uniform zero-padded ` (nn)`, per-group width).
- `plan`: given a tree of names, timestamps, identifiers, content hashes, and a policy (budget, root length), produce every final path with flags and counters.

Gate:
- Unit tests for every rule, including astral-plane characters counted as two units, a directory named `NUL`, trailing dots and spaces, NFC versus NFD names, case-only differences, empty names, `folder.json` and `.tsp-tmp` collisions, attachment-file versus embedded-directory collisions, and suffix widths (a natural `Budget (02)` surviving; groups of 2, 99, 100, 999, 1,000, and 1,500).
- **Property tests** (`proptest`) over arbitrary names and trees: sanitizing is idempotent; no final component contains a reserved character; every path is within budget; no two entries in a namespace share a collision key; a plain text sort of a group's final names equals its (time, identifier) order; output is identical across runs and independent of input order.
- Each research check (C-01 to C-18 as applicable) exists as a named plan counter.

### M4b-3 — naming census and folder-identity spike (counts only)

Add `tsp <input> --out <dir> --dry-run`: runs the planning phase over a PST or `.msg` input and prints only counts in the stable `key=value` vocabulary, with `plan_gate_violations`, in the `--verify` style. It writes nothing.

Keys (proposed): names needing sanitization by class; reserved-name hits; trailing-dot and trailing-space hits; empty subjects; components truncated; maximum component and path lengths in UTF-16 units; maximum relative path; maximum folder depth; folders flattened; collision groups (messages, folders, attachments); the largest collision group and the number of groups of 100 or more; subdirectories mirrored (directory inputs); messages missing `PidTagInternetMessageId`; duplicate `PidTagInternetMessageId`; attachments without names; attachment name collisions; budget exceeded; maximum entries per directory.

Spike: determine what `outlook-pst` v1.2.0 exposes as a folder identifier (NID, `PidTagRecordKey`, EntryID) and record it.

Gate: the census runs on the 29-message `.msg` set, the 57-message PST, and every fixture available; the counts are reviewed with the owner; the folder-identity question is answered (available, not available, or partly).

### M4a-2 — accept the new ADRs

Using the census, accept up to five ADRs that **complete** the archive ADR: (1) export naming, collisions, and path budgets; (2) message and folder identity and provenance; (3) output posture (`--out`, `--dry-run`, consent, ownership by `folder.json`); (4) body and formatting-loss policy; (5) `metadata.json` / `folder.json` schemas and per-item status. The archive ADR gets a cross-reference in "More Information" (status and layout unchanged).

Gate: ADRs accepted by the owner; a hand-written example archive for one corpus message (private content replaced) that follows them and passes the plan's own invariants.

### M4b-4 — normalized model types

`OutlookItem` and parts: provenance, properties (typed, raw bag preserved), recipients, bodies (each variant with raw bytes and detected encoding), attachments (data source and status), embedded items, named properties, diagnostics. Pure data.

Gate: invariants tested; a table mapping each existing counter category to its model field.

### M4c — walking skeleton on synthetic input

A thin path end to end for the simplest case: a synthetic plain-text `.msg` (built at test time, as the ANSI tests do) with no attachments through model, plan, consent, staged write, and read-back, producing `<stem>/<subject>/message.md`, `metadata.json`, and the root and folder `folder.json`.

Includes the overwrite behavior: counts-only preflight, interactive prompt, non-interactive refusal without `--overwrite`, refusal to touch anything the tool did not create, and `target_source_mismatch` detection.

Gate:
- Golden files committed for synthetic fixtures (no personal data), run in CI on Linux and Windows.
- Two exports of the same input are byte-identical (tree hash).
- Overwrite matrix tested: empty target, identical existing files, different existing files, unrelated files, non-interactive without consent.
- One real corpus message exported and reviewed by the owner against a checklist.

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

Staged, atomic writes via the short fixed `.tsp-tmp` staging directory; replace-on-rename behavior verified on Windows; deterministic ordering and line endings (LF, UTF-8 without BOM); modification-time policy; partial-failure behavior (one failed message never corrupts or hides others); FAT32 directory-size warning; `--long-paths` and `--max-relative-path` options.

Gate: run-twice byte-identical trees on the corpus (single tree-hash line); fault injection (unwritable target, simulated disk full where practical, corrupt input mid-batch); results independent of input order; consent flow re-tested.

### M4i — PST into the model

Known constraint P4c: attachment bytes and embedded-message content are unreachable through `outlook-pst` v1.2.0's public API. Options: (1) accept and flag: archive attachment metadata, mark bytes not extracted (recommended first); (2) go below the public API; (3) raise the gap upstream (recommended in parallel).

Work: PST messages through the same body pipeline; folder hierarchy into the layout; folder identity per the spike; ANSI PST support if `outlook-pst` provides it.

Gate: the diagnostic's counts reconcile with the archive (57 messages and 79 attachments accounted for as extracted or flagged); plan counters match census counters; a `libpff` or `libpst` differential is scoped even if executed later.

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

Results reach me as the output of your runs (`tsp <pst>`, then the census), plus a short note on how each was made; the PST files do not need to be sent. Priority is by risk removed. At least 1-3 are wanted before M4b-3.

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

## Principal risks

- **Naming policy** (M4a/M4b). Naming decides re-export stability; a wrong choice forces re-exporting archives. Mitigation: pure engine, census before the ADRs are accepted, property tests.
- **Module split without a compiler** (M4b-1). Visibility, import, and test-placement errors are likely on the first build. Mitigation: a mechanical, scripted split; an equivalence check of item bodies before delivery; the owner's compiler output drives fixes.
- **Path budget in a directory-per-message layout.** Every message is a directory, so the 248 rule and worst-case child reservation bind tightly; long subjects plus long attachment names will be truncated often on long roots. Mitigation: budget from the real root, report truncation counts, `--max-relative-path`, guidance to export near a drive root.
- **Bodies** (M4e). De-encapsulation and HTML-to-Markdown quality on Word HTML; partly subjective. Mitigation: bake-off, content-preservation check, verbatim body kept.
- **PST attachments** (M4i). Blocked by the public API.
- **Folder identity.** The NID-derived identifier may not be exposed by `outlook-pst`; the fallback ordering is deterministic but weaker.
- **Content-hash definition** (Q11) now affects only the `identical_to` metadata, not any name, so a poor first definition is cheap to correct.
- **Very large duplicate groups.** Thousands of same-subject messages in one folder widen their suffix and rename the group once when it crosses 99 or 999 members. Mitigation: per-group width, census counts of large groups.
- **Uninformative file names.** Every message is called `message.md`, so search results and editor tabs show the folder only as context. Accepted with the ADR; `message.md` can open with a title line (body-policy ADR).
- **Single-producer corpus.** Addressed by the fixture table.
- **Windows-only behavior** is verifiable only on Windows; CI covers unit tests on Linux and Windows.
- **Dependencies.** Each new crate is a maintenance risk; pin, read source, check license.

## Decisions still needed from the owner

1. Fixtures 1-3 before M4b-3 (the owner is creating them).
2. Q11, the content-hash definition, before M4g (not before).
3. First build of the M4b-1 module split, with compiler output for any errors.
