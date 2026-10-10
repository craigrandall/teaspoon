# M4 plan (v4.10) — normalized model and deterministic Markdown archive

Status: **M4a-1 decided; M4b-1, M4b-2, and M4b-3 verified (v0.1.22, 2026-10-02) with open items listed in the M4b-3 section; M4c complete (tag v0.1.23.1, 2026-10-03); M4a-2 complete for the four ADRs that could be decided (accepted by the owner 2026-10-04, release v0.1.24, documentation only); M4d (the envelope, with the envelope part of M4b-4) closed 2026-10-08 (code v0.1.25, follow-ups v0.1.26 and v0.1.26.2; 0 mismatches against `msg_parser` over 34 messages; every property identifier confirmed against Microsoft's pages); M4e-1 (RTF de-encapsulation) built, with the recognition rule confirmed on the corpus and the recovered HTML differing from `msg_parser`'s in all 27 pairs, untriaged; the body and formatting-loss ADR is deferred to M4e.** Version 4.10, revised 2026-10-08 from version 4.9 to record `v0.1.26.2`: the corrections applied, the first corpus runs of `--verify-envelope --recursive` and `--verify-deencap`, M4d closed, and the M4e-1 content differential open. Every stage has an evidence gate that must be met on Windows before the stage counts as done. Companion documents:

- [`m4a-export-rules.md`](m4a-export-rules.md): the naming, layout, identity, duplicate, path-length, and overwrite rules (v3.6; sections 4-6 are now backed by an accepted ADR).
- [`m4a-dependency-research.md`](m4a-dependency-research.md): RTF de-encapsulation, HTML-to-Markdown converters, and supporting crates.
- [`m4d-envelope-properties.md`](m4d-envelope-properties.md): the envelope's property identifiers, how well each is confirmed, and what `--verify-envelope` compares.
- [`../verification/m4-results.md`](../verification/m4-results.md): the evidence recorded so far.
- [`../examples/m4c-example-archive.md`](../examples/m4c-example-archive.md): a worked example of the draft format against the accepted ADRs.

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
- 2026-10-03: the owner directed that M4c proceed ahead of M4a-2 (the ADRs) and M4b-4 (the model types), which this plan places before it. M4c used a small seed of the model and **draft** schemas so that the ADRs could decide what the skeleton had actually produced.
- 2026-10-04: the owner directed that M4a-2 be taken next, read the four ADRs, and **accepted each of them**, including the decisions that went beyond the original overwrite instruction (an archive of a different source is refused even with `--overwrite`; source identity is kind plus name, not hash; a non-empty directory without a `tsp` marker is refused; per-file rather than whole-archive atomicity for now) and the binding file rules (UTF-8 without BOM, LF, fixed field order, no absolute paths or export-time values).
- 2026-10-07: the owner reported the v0.1.25.1 corpus tree hash and directed that M4d's loose ends be closed and M4e-1 (in-house MS-OXRTFEX de-encapsulation) started now, ahead of the HTML-to-Markdown bake-off (M4e-0), which needs real HTML that only M4e-1 can supply from this corpus.
- 2026-10-06: the owner took M4d next, together with the envelope part of the model (M4b-4), ahead of the bake-off (M4e-0), and built and tagged it (v0.1.25, v0.1.25.1).

## What changed from v4.9

1. **`v0.1.26.2` recorded** (203 tests, built, tagged): the `source_pst.rs` duplicate removed (confirmed by a PST dry run with a short `--out`: 154 entries, 10 folders, 60 messages, 84 attachment files, 0 gate violations), the duplicated tally print removed, and recipient-type flags ignored when classifying.
2. **M4d is closed.** The differential run over all 34 messages (recursive) has 0 mismatches; the email-property question is answered for this corpus; every property identifier is tier A; `fmt`, `clippy`, and CI succeeded for the tags. Residual items are listed in the M4d section.
3. **M4e-1: recognition confirmed, content not.** The 10-token rule agrees with the whole-document search on all 29 files, and `msg_parser` and the custom path agree on which 27 messages have HTML. But the 27 recovered HTML bodies differ from `msg_parser`'s in every file at every agreement grade, so the differential gate is not met and the differences are untriaged. One undecodable byte (code page 1255) in 1 file.

## What changed from v4.8

1. **`v0.1.26` recorded** (2026-10-08): M4e-1 and the M4d follow-ups built after the owner fixed several build issues and moved the crate to Rust edition 2024 with all dependencies updated; 201 tests pass; CI succeeded. The owner's revisions to `rtf_deencap.rs`, `verify_deencap.rs`, `verify_envelope.rs`, and `export.rs` were reviewed (see `m4-results.md`): a real bug in the drafted `rtf_deencap.rs` was fixed (a trailing backslash produced a literal backslash), a useful breakdown was added to `--verify-deencap`, and the rest is edition-2024 and `rustfmt` style.
2. **A defect was found in the review:** in `source_pst.rs`, the loop over a folder's sub-folders appears twice (a duplicated block from the migration). The PST dry run and any future PST export would plan every non-root folder more than once, compounding with depth; the 201 tests do not cover it because the PST fixture is not in the repository. It needs a one-block deletion and a PST dry run to confirm (expected: the same 10 folders, 60 messages, 84 attachment files as before).
3. **M4d identifiers:** every property identifier is now tier A (seen on Microsoft's pages, 2026-10-07). A likely defect in recipient handling was found (a recipient type with the MAPI_P1 or MAPI_SUBMITTED flag would be dropped from the list) and is not yet fixed.
4. **Quality gate for M4d:** `fmt`, `clippy`, and CI succeeded for `v0.1.25`, `v0.1.25.1`, and `v0.1.25.2`. The PST dry run with a short `--out` was reported and is clean (0 gate violations).

## What changed from v4.7

1. **M4d baseline recorded.** The corpus export at `v0.1.25.1` (and `v0.1.25.2`, documentation only) has `export_tree_sha256=72d8553417441719c7c1e2b1d3ec253141fb5c2697c311d4210a92cd5d71c2e5`.
2. **M4d loose ends:** three property identifiers (0x0064, 0x0065, 0x39FE) are tier A (seen on Microsoft's MAPI canonical property pages); the email tally split ("SMTP absent" versus "present and different") and `--recursive` for `--verify-envelope` are written, not yet run; `fmt`, `clippy`, and CI for the tag are still not reported. M4d stays "built and verified for the comparable fields", not closed.
3. **M4e-1 written, not built or run:** `rtf_deencap.rs`, `verify_deencap.rs` (`--verify-deencap`), and edits to `cli.rs`, `main.rs`, and `verify_envelope.rs`. Its gate is not met until the owner builds it and reports a run.
4. The PST short-`--out` item is unchanged: the run reported (without `--dry-run`) was the designed "not implemented" refusal; it needs `--dry-run`.

## What changed from v4.6

M4d was recorded. The envelope (sender, sent-representing, To/Cc/Bcc recipients, times, importance, sensitivity, conversation fields, transport headers) is now read, held in the model, shown in `message.md`, and recorded in `metadata.json`; `--verify-envelope` compares it with `msg_parser`. Evidence: 165 tests pass; `verify-split.ps1` outputs are identical to the pre-split baseline; the differential run over the 29 top-level `.msg` files had 0 mismatches. The status reason `envelope_not_extracted` became `other_properties_not_preserved`, and the `hello` golden files changed. M4b-4 is partly done (the envelope types). The "Where the code stands", M4b-4, M4d, risks, and "Decisions still needed" sections were revised; M4d is **not** recorded as closed, because the plan's requirement that every property identifier be confirmed against Microsoft's specifications is only partly met.

## What changed from v4.5

The owner accepted the four ADRs on 2026-10-04. Their status moved from proposed to accepted; the archive ADR's cross-reference now says the follow-on ADRs are accepted (its own status and decision are unchanged). The "decisions still needed" list lost its first item. No code changed; the release is v0.1.24, documentation only.

## What changed from v4.4

M4a-2 was drafted. Four ADRs were written as proposed (naming, identity, output posture, archive file contract); the fifth (body and formatting-loss policy) was not drafted because it depends on the HTML-to-Markdown comparison (M4e-0) and would otherwise be invented. The fifth ADR in the earlier list ("`metadata.json` / `folder.json` schemas and per-item status") was narrowed to the *rules every archive file must follow and how schemas evolve*; the field-level schemas stay drafts until M4g, because they depend on work not done yet. A worked example archive was added.

## What changed from v4.3

M4c is closed: `cargo fmt --check` and `cargo clippy -D warnings` reported no errors, CI succeeded, and the non-interactive refusal (exit code 2) and `--overwrite` were verified on real output. The outstanding-reports list is gone; what remains open is the PST items from M4b-3 and the two housekeeping items.

## What changed from v4.2

M4c's real-corpus gate item was recorded as met (counts reconcile, repeat export writes nothing, tree hash identical across runs and output directories, the owner's review of an exported message passed, the interactive overwrite prompt works on real output). The "export untested on real messages" risk was narrowed accordingly.

## What changed from v4.1

1. M4c was built; its status, behavior, and the decisions taken in it are recorded in the M4c section.
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

## Where the code stands (v0.1.25.1; v0.1.24 added documentation only)

- The MSG adapter decodes typed properties, value streams, named properties, recipients, attachments (methods, content IDs, emptiness), one level of embedded message, and, since M4d, the envelope (`oxmsg_envelope.rs`). It reads content into memory and prints only counts.
- The PST adapter is a counter-only diagnostic over `outlook-pst` v1.2.0. It cannot open embedded-message or OLE attachments (P4c).
- `msg_parser` is an oracle for comparable MSG fields (`--verify`, and `--verify-envelope` for the envelope); no PST oracle has been run.
- The pure naming rules and export planner exist (`naming.rs`, `plan.rs`), and `--dry-run` plans an export of a PST, a `.msg`, or a directory of `.msg` files and prints a content-free census.
- **`tsp <msg|dir> --out <dir>` exports** `.msg` input: `message.md` (the subject as a heading, a list of the envelope fields the message has, and the plain-text body in a code fence), `metadata.json` (including an `envelope` object), and `folder.json` files, with preflight, consent, staged writes, and read-back. The schemas are drafts (`0.1-draft`). The model holds the message content and the envelope (`model.rs`); the other bodies, attachment bytes, the remaining properties, and PST export do not exist. 165 tests pass. The export was run on the real `.msg` corpus at v0.1.23.1; it has not been reported for v0.1.25.1.
- Fixtures: 29 `.msg` files at the top of the fixture directory (27 carry HTML only inside RTF; 23 attachments carry a content ID; 1 embedded message; 1 zero-byte attachment; no `PT_STRING8` value), more `.msg` files in 3 subdirectories (which the recursive dry run and export include, and the non-recursive `--verify` and `--verify-envelope` do not), and one enhanced PST.

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
- **`plan_gate_violations=0` and `plan_budget_exceeded=0` on both inputs: met for the `.msg` directory, at the long output path (census) and at a short one (the M4c export); not met for the PST at the long output path used** (4 over-budget entries at a root of 152 units, longest path 266 against 259). This is the budget gate working as designed under a deliberately long `--out`; the PST still has to be shown at a short `--out`, which has not been run.
- **Predicted counts: not matched.** Predicted PST `plan_folders=9`, `plan_messages=57`, `plan_attachment_files=79`; census 10 / 60 / 84. Predicted `.msg` directory 29 / 1 / 29; census 34 / 3 / 34, probably because the recursive scan includes 3 subdirectories (not itemized). The PST difference is unreconciled; it needs the current PST diagnostic output.
- The counts are reviewed with the owner, and the fixtures received so far are added: the counts were reviewed on 2026-10-03; whether the fixture set changed since the earlier diagnostic is not established.
- The folder-identity question is answered from the run (above).
- Output of the existing modes is unchanged: met (`verify-split.ps1`: default, `--verify`, and PST outputs IDENTICAL to v0.1.19; repeated on v0.1.23.1 and on v0.1.25.1).

### M4a-2 — accept the new ADRs

Status: **complete for four ADRs (drafted and accepted by the owner on 2026-10-04); the fifth is deferred.** The ADRs *complete* the archive ADR and do not supersede it; the archive ADR carries a cross-reference (its status and decision unchanged).

| ADR | File | Status |
|---|---|---|
| Export naming, collisions, and path budgets | `ADRs/export-naming-collisions-and-path-budgets.md` | accepted |
| Message and folder identity and provenance | `ADRs/message-and-folder-identity-and-provenance.md` | accepted (the PST identifiers and the content hash are decided but not yet written by any code) |
| Export output posture: consent, ownership, and counts-only output | `ADRs/export-output-posture-consent-and-ownership.md` | accepted |
| Archive file contract and schema evolution | `ADRs/archive-file-contract-and-schema-evolution.md` | accepted (decides the file rules and the evolution policy; field-level schemas stay `0.1-draft` until M4g) |
| Body and formatting-loss policy | — | **not drafted**: it needs the HTML-to-Markdown comparison (M4e-0) and the de-encapsulation work (M4e-1); M4c's fenced-text body is explicitly provisional |

Following the ADR guidance in `ADRs/_README.md` (an ADR records a decision that is expensive to reverse, constrains later work, and had real alternatives), each ADR lists its alternatives, states what is confirmed by evidence and what is not, and records the status as the decision's, not the work's.

Gate: ADRs accepted by the owner — **met for the four** (2026-10-04); a hand-written example archive that follows them and passes the invariants — **written** as `docs/examples/m4c-example-archive.md` (the message files are the committed golden files; the two `folder.json` files are hand-assembled with placeholder numbers), with the invariants checked by hand in the document. The example is synthetic: it is not a real corpus message.

Follow-ups that the accepted ADRs create: rule L4 steps 4 and 5 (naming ADR); recording folder and PST message identifiers and the content hash, once Q11 is decided (identity ADR); whole-archive atomicity and streaming (output-posture ADR, M4h); the closed status-reason vocabulary and the first numbered `schema_version` (file-contract ADR, M4g).

### M4b-4 — normalized model types

`OutlookItem` and parts: provenance, properties (typed, raw bag preserved), recipients, bodies (each variant with raw bytes and detected encoding), attachments (data source and status), embedded items, named properties, diagnostics. Pure data.

Status: **partly done.** `model.rs` holds `MessageContent` (subject, Internet message ID, time, plain-text body, which other body forms exist) from M4c, and since M4d the envelope types `Envelope`, `Address`, and `Recipient`. Bodies with raw bytes, attachments, the raw property bag, named properties, embedded items, and diagnostics are not modelled yet.

Gate: invariants tested; a table mapping each existing counter category to its model field. Not met yet.

### M4c — walking skeleton

Status: **complete (v0.1.23, tag v0.1.23.1, 2026-10-03, 155 tests).** New modules `model.rs`, `archive.rs` (pure rendering), `export.rs` (preflight, consent, staged write, read-back); `source_msg.rs` gains the message-to-file map and `read_message_content`; `cli.rs` gains `--overwrite` and `--no-source-hash`; `root_stem` ignores trailing separators. New dependencies: `serde`, `serde_json`, `sha2`.

What it does: for `.msg` input (a file, or a directory handled recursively), plan names with the M4b planner and refuse a plan that breaks a gate; render the archive in memory (`message.md`, `metadata.json`, `folder.json`, root `folder.json`); compare it with the target (counts only); decide consent; write each file through `<out>/.tsp-tmp/<n>` and rename it into place; read everything back; print `export_*` counts and one `export_tree_sha256`. Attachments and embedded messages are counted and recorded as not extracted, and no `attachments/` directory is written. `.pst` input stops with a "not implemented" error (M4i).

Decisions taken in M4c (written up in the output-posture and file-contract ADRs, which the owner accepted on 2026-10-04):
- **Consent.** Replacing a file the tool generated earlier needs `--overwrite` or an interactive yes; non-interactive without it, the run is refused (exit code 2). A target directory the tool did not create is refused even with `--overwrite`. An archive recorded for a different source (different kind or name) is refused even with `--overwrite`; the source hash is deliberately not part of source identity, so re-exporting a changed source is allowed with consent.
- **Interrupted runs.** The root `folder.json` is written first as `incomplete` and replaced with the `complete` version last. Whole-archive atomicity is not provided; per-file replacement is atomic.
- **Staging.** A short numeric name under `.tsp-tmp` (as planned for M4h), with the `--out` path limited so staged paths cannot exceed final ones. Leftover numeric files from an interrupted run are removed; anything else in `.tsp-tmp` is an error.
- **Body.** Provisional: the subject as a heading, the plain-text body verbatim in a code fence (longer than any backtick run in the body). The body policy ADR (M4e) decides the final form.
- **Status.** Every message is `partial`, with reasons. At M4c the reasons were: envelope not extracted; attachments not extracted; formatted bodies not converted. Since M4d the always-present reason is `other_properties_not_preserved` instead of `envelope_not_extracted`.
- **Housekeeping not done:** `#![allow(dead_code)]` in `naming.rs` and `plan.rs`; the `Cargo.toml` version was `0.1.23` while the tag is `v0.1.23.1`.

Gate (all met):
- Golden files committed for synthetic fixtures (no personal data), run in CI on Linux and Windows: **met** — committed, passing locally on Windows, and the CI workflow succeeded on the pushed commit.
- Two exports of the same input are byte-identical (tree hash): **met** — synthetic test, and on the real `.msg` corpus the tree hash `96da290e…793ad` was identical across a first run, a repeat into the same directory, and a run into a different directory.
- Overwrite matrix tested (empty target, identical existing files, different existing files, unrelated files, non-interactive without consent, foreign directory, other source, over-budget plan): **met** by synthetic tests, and on real output the interactive prompt (accepted), the non-interactive refusal (exit code 2, nothing written), and `--overwrite` were each exercised; the edited file was restored every time, with an unchanged tree hash.
- One real corpus message exported and reviewed by the owner against a checklist: **met** — the whole `.msg` corpus was exported (34 messages, 72 files, 0 read-back mismatches, longest written path 131 units) and the owner reviewed one `message.md` / `metadata.json` pair and found the contents as expected. The other exported messages have not been individually reviewed.
- Quality gate: `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` reported no errors.

### M4d — envelope: headers, recipients, properties

Status: **closed 2026-10-08 (code `v0.1.25`; tags `v0.1.25.1`, `v0.1.25.2`; follow-ups `v0.1.26` and `v0.1.26.2`; 203 tests).** New modules `src/oxmsg_envelope.rs` (extraction) and `src/verify_envelope.rs` (`--verify-envelope`, with `--recursive` and the split email tally); `model.rs`, `archive.rs`, `source_msg.rs`, `cli.rs`, and `main.rs` changed. The identifiers, their confirmation, and the extraction rules are in [`m4d-envelope-properties.md`](m4d-envelope-properties.md).

What it does: the sender, sent-representing identity, To/Cc/Bcc recipients (each with display name, address type, address, and SMTP address, kept separate and not repaired; the optional MAPI_P1 and MAPI_SUBMITTED flags on the recipient type are ignored when classifying), submit and delivery times, importance, sensitivity, conversation topic and index, and transport headers are read, held in the model, and written. `message.md` shows a header list; `metadata.json` carries an `envelope` object. `--verify-envelope` compares the subject, sender, and recipients with `msg_parser`, counts only.

Evidence (details in `m4-results.md`): `fmt`, `clippy`, and CI succeeded for the tags; the corpus export tree hash at `v0.1.25.1` is `72d8553417441719c7c1e2b1d3ec253141fb5c2697c311d4210a92cd5d71c2e5`; `verify-split.ps1` outputs identical to the baseline at `v0.1.25.1`; `--verify-envelope --recursive` over 34 files: 0 mismatches on every comparable field, and in all 5 cases where the two email properties differ `msg_parser` chose the SMTP address, matching `message.md`'s rule.

Gate: met. The differential check covers every field `msg_parser` exposes; every property identifier is confirmed against Microsoft's pages; unit tests and inspection cover the rest. Residual, not blocking: importance low = 0 and normal = 1 are not in Microsoft text seen; sent-representing, times, importance, sensitivity, conversation fields, and transport headers have no oracle; `verify-split.ps1` and a corpus export tree hash have not been reported for `v0.1.26.2` (the format did not change, so `72d85534…c2e5` should still hold); the corpus is one producer.

### M4e-0 — HTML-to-Markdown bake-off (approved)

Follow [`m4a-dependency-research.md`](m4a-dependency-research.md): candidates `htmd`, `html2markdown`, `html-to-markdown-rs`, behind an `HtmlToMarkdown` trait, judged on determinism, panics, content preservation, tables, links and images, licensing, dependency weight, and custom-handler support.

Gate: a recorded choice with the exact version pinned, in the body-policy ADR.

Note for sequencing (not yet decided): the `.msg` corpus has no native HTML body (27 of 29 messages carry HTML only inside RTF), so a bake-off on real data needs the de-encapsulation of M4e-1, or new fixtures with native or Word-authored HTML (fixture priority 10).

### M4e-1 — MS-OXRTFEX de-encapsulation (in-house)

Status: **built and unit-tested (`v0.1.26`, corrected in `v0.1.26.2`; 203 tests; CI green); corpus differential run, recognition confirmed, content differences untriaged.** New modules `src/rtf_deencap.rs` (pure) and `src/verify_deencap.rs`; `cli.rs` gains `--verify-deencap` and `--recursive`. No dependency was added. The owner's revisions beyond the drafted code: a `TrailingBackslash` token (the draft emitted a literal backslash for a lone trailing backslash), a `recognition_mismatch_by_kind` breakdown in `--verify-deencap`, let-chains and import ordering from the edition-2024 migration.

The specification pages this was written from (all read 2026-10-07): MS-OXRTFEX 2.2.3.1 (recognizing encapsulation: `{\rtf1` first; at most the first 10 tokens, begin-group marks and control words; `\fromhtml1` or `\fromtext` among them; any other token kind, or neither word, means ordinary RTF), 2.2.3.2 (extracting encapsulated HTML), 2.1.3.1.4 (the HTMLTAG destination group, whose numeric parameter is ignored), and 2.1.3.1.5 (the MHTMLTAG group, covered by the rule that ignorable destinations other than HTMLTAG are skipped).

Decisions taken in the code, each stated so it can be overruled:
- **Recognition is the 10-token rule**, stricter than the whole-document `\fromhtml1` search in `shared.rs`. The two are compared by `--verify-deencap`; `shared.rs` is not changed.
- **Output is UTF-8 text.** A `charset=` in the recovered HTML is stale; `meta_charset_declared` records it and the body pipeline (M4e-2) must not trust it.
- **`htmltag` content** is decoded in the document's default code page; **ordinary text** in the current font's code page (from `\fcharset` or `\cpg` in the font table); the font is tracked inside `\htmlrtf` regions, as the specification says.
- **`\htmlrtf` is group-scoped** like any RTF character-formatting toggle. This is an assumption; a corpus disagreement with `msg_parser` that traces to it would show here first.
- **Code pages** are limited to those `oxmsg_decode` implements (1252, ISO-8859-1, US-ASCII, UTF-8). Anything else becomes U+FFFD per byte and is counted (`deencap_undecodable_bytes_total`, `deencap_unsupported_codepage_<n>_files`); no code page crate is added until the corpus shows it is needed.
- **Skipped destinations** are a fixed list of standard destinations without visible text, only as a group's first word; unlisted ones are treated as visible (a known limit).
- **Bounds:** group depth 1024 and output 64 MiB, each reported as a flag when hit; no input can panic it (a property test covers arbitrary bytes).
- The decompressed RTF still comes from `compressed-rtf`, whose result is a string; an RTF stream with raw non-UTF-8 bytes would fail decompression and be counted (`rtf_decompression_failed`). Replacing it with an in-house MS-OXRTFCP decompressor is a possible follow-up if the corpus shows it matters.

Gate: spec-derived unit and golden tests: **met**. Property test that arbitrary input never panics and stays within the output bound: **met**. Recognition rule confirmed against `check_compressed_rtf_bytes`: **met on this corpus** (0 disagreements in 29 files). Weak-oracle differential against `msg_parser`'s `html_from_rtf()` on the 27 encapsulated-HTML messages, counts only, every disagreement triaged: **not met**: presence agrees on all 29 files, but the recovered HTML differs from `msg_parser`'s in all 27 pairs at every agreement grade (sizes within 0.24% in total, deltas −1571 to +1586 bytes, three at +15), and no difference has been triaged. Agreement is graded, not pass/fail: byte-identical, equal ignoring whitespace, equal ignoring whitespace and non-ASCII, same visible text, same visible text ASCII only, different.

Other results from the first run: 1 undecodable byte in 1 file (code page 1255 unsupported), so no code page crate is warranted yet; no file declares a `charset=`; no limit, unclosed-group, or bad-escape counts. One message is `\fromtext` (encapsulated plain text), which this module does not handle (the plain-text body already exists).

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

Implement the `metadata.json` and `folder.json` schemas under the accepted file-contract ADR: identity and provenance (per the accepted identity ADR), property bag, recipients, attachment records, named properties, extraction diagnostics, per-item status (complete, partial, failed, each with a closed list of reasons), applied renames and truncations, and the folder's child index, and the content SHA-256 with `identical_to` (Q11, the only open definition). Stdout summary of counts. Freezing the first numbered `schema_version` happens here, under the evolution policy in the file-contract ADR. (The envelope object is already written as a draft by M4d.)

Gate: JSON schema tests on synthetic archives; the corpus stdout summary reconciles with `--verify` totals (every message accounted for); no absolute paths and no export-time values in any metadata file.

### M4h — writer hardening

Whole-archive atomicity or an equally clear recovery story (M4c writes per file and marks the root `incomplete` until the end); streaming instead of rendering the whole archive in memory; replace-on-rename behavior verified on Windows (M4c verified it on real output); deterministic ordering and line endings (LF, UTF-8 without BOM); modification-time policy; partial-failure behavior (one failed message never corrupts or hides others); FAT32 directory-size warning; `--long-paths` and `--max-relative-path` options.

Gate: run-twice byte-identical trees on the corpus (single tree-hash line); fault injection (unwritable target, simulated disk full where practical, corrupt input mid-batch); results independent of input order; consent flow re-tested.

### M4i — PST into the model

Known constraint P4c: attachment bytes and embedded-message content are unreachable through `outlook-pst` v1.2.0's public API. Options: (1) accept and flag: archive attachment metadata, mark bytes not extracted (recommended first); (2) go below the public API; (3) raise the gap upstream (recommended in parallel).

Work: PST messages through the same body pipeline; folder hierarchy into the layout; folder identity per the spike and the accepted identity ADR (both identifiers were available on the fixture); ANSI PST support if `outlook-pst` provides it; the PST envelope (the M4d envelope reads the `.msg` recipient storages and message properties; the PST side needs its own reader, symmetric where the API allows).

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
| 10 | **More `.msg` files** | Bodies (M4e); also the envelope paths the corpus never exercises (an `EX` sender with no SMTP address, a message sent on behalf of someone, an originator or unlisted recipient row) | Word-authored HTML with tables and colors; signature images; a genuine RTF-only message; ANSI-encoded messages; messages from other clients saved as `.msg`; messages from an Exchange mailbox |

Privacy: any of these can be synthetic, and census outputs are counts, so they are safe to paste. Do not paste subjects or names.

## Architecture note: separate crates (not a goal now)

The module split exists for navigability and so that new pure modules (`naming`, `plan`, the model) can be developed and tested without the adapters. It is not a first step toward a Cargo workspace. All items are `pub(crate)` because the split was mechanical, so the current boundaries are not a designed public API, and the crate is `publish = false`.

Crate boundaries become worthwhile only for a concrete reason: another program embedding the engine, enforcing that the model never depends on an adapter, isolating heavy dependencies (`outlook-pst`, `msg_parser`, an HTML converter) so they compile and change separately, or independent versioning. None applies today. Decision: stay a single binary crate through M4, keep `naming`, `plan`, and the model free of imports from `pst`, `oxmsg_*`, and `verify` (a test can check the `use crate::` lines), and revisit at M4j with evidence such as build times and the shape of the interfaces. If the boundary is ever drawn, a plausible set is: model, naming, and plan; the MSG adapter; the PST adapter; the archive writer; and the CLI with `--verify`.

## Principal risks

- **Naming policy** (M4a/M4b). Naming decides re-export stability; a wrong choice forces re-exporting archives. Mitigation: pure engine, census, property tests, and now an accepted ADR with its open confirmations listed.
- **Path budget in a directory-per-message layout.** Every message is a directory, so the 248 rule and worst-case child reservation bind tightly; long subjects plus long attachment names will be truncated often on long roots. The PST fixture already shows entries that cannot fit at a root of 152 units (4 of 154). Mitigation: budget from the real root, report truncation counts, `--max-relative-path`, guidance to export near a drive root. M4c refuses to export a plan that breaks a gate; whether a better answer exists (flattening, L4 step 4) is open.
- **Bodies** (M4e). De-encapsulation and HTML-to-Markdown quality on Word HTML; partly subjective. Mitigation: bake-off, content-preservation check, verbatim body kept. The corpus has no native HTML, so real-data evidence for the converter depends on the de-encapsulation or new fixtures.
- **PST attachments** (M4i). Blocked by the public API.
- **PST counts unreconciled.** The planner and the earlier PST diagnostic disagree on the fixture's size (see M4b-3); until resolved, the M4i reconciliation gate has no trusted baseline.
- **Folder identity.** Both identifiers were readable on the fixture; the NID-derived identifier is still unproven across other producers. The fallback ordering is deterministic but weaker.
- **Content-hash definition** (Q11) now affects only the `identical_to` metadata, not any name, so a poor first definition is cheap to correct.
- **Very large duplicate groups.** Thousands of same-subject messages in one folder widen their suffix and rename the group once when it crosses 99 or 999 members. Mitigation: per-group width, census counts of large groups (the fixture's largest group is 16).
- **Schemas frozen too early or too late.** M4c and M4d produce draft schemas ahead of the field-level freeze on purpose; the risk is treating them as final. They carry `schema_version` `0.1-draft`, and the accepted file-contract ADR says drafts carry no compatibility promise.
- **Envelope identifiers and meanings partly unconfirmed** (M4d). Some property identifiers (tier B and C) and the importance and sensitivity meanings are not confirmed against Microsoft's text; the sent-representing, time, importance, sensitivity, conversation, and header values have no oracle. The `msg_parser` comparison agreed on the comparable fields, but it is one corpus and one oracle whose email-selection rule is not fully known. Mitigation: confirm the identifiers against MS-OXPROPS, split the email tally, add fixtures that exercise the unexercised paths (`EX` without SMTP, unlisted rows).
- **ADRs accepted ahead of the evidence they need.** The identity ADR decides things (PST identifiers in metadata, the content hash) that no code writes yet; the naming ADR depends on rules L4 steps 4-5, which are not implemented. The ADRs say so in their Confirmation sections, and the follow-ups are listed in M4a-2.
- **Export reviewed on a small sample.** The real corpus exported cleanly at v0.1.23.1 and one message was reviewed there; at v0.1.25.1 a couple of messages were examined and no corpus export was reported. The corpus is one producer; large bodies, unusual encodings, and long subjects beyond the fixtures are untested.
- **Memory use.** The writer renders the whole archive in memory first; a very large mailbox will need the streaming work in M4h.
- **Uninformative file names.** Every message is called `message.md`, so search results and editor tabs show the folder only as context. Accepted with the ADR; `message.md` can open with a title line (body-policy ADR), and M4c's draft does.
- **Single-producer corpus.** Addressed by the fixture table.
- **Windows-only behavior** is verifiable only on Windows; CI covers unit tests on Linux and Windows.
- **Dependencies.** Each new crate is a maintenance risk; pin, read source, check license.

## Decisions still needed from the owner

1. **Triage the 27 HTML differences** (the open M4e-1 gate item). `--dump-deencap <dir>` is written (not yet built or run): with `--verify-deencap` it writes each message's custom HTML, `msg_parser` HTML, and decompressed RTF into a new or empty directory you name. Then diff one or two pairs locally (start with a pair whose size difference is exactly 15 bytes; three pairs share it) and describe the difference to the assistant without content.
2. The current `tsp tsp-tester.pst` diagnostic output, to reconcile the PST counts (60/84 against the earlier 57/79). The planner is stable at 154 entries, so the open question is the diagnostic's side.
3. Whether to run `verify-split.ps1` and a corpus export at `v0.1.26.2` to confirm the outputs and the tree hash did not move.
4. Whether to try removing `#![allow(dead_code)]` now, and the `Cargo.toml` version (the tags go past it).
5. A code page crate: not warranted on this corpus (1 byte, 1 file); revisit with a second producer's fixtures.
6. Q11, the content-hash definition, before M4g (not before).
