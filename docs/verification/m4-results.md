# M4 verification results

Evidence for the M4 stages, recorded as it arrives. Only what was actually run and reported is stated as fact; open questions are listed as such. All outputs are content-free counts. Counts come from the owner's runs on Windows.

## M4b-1: module split (v0.1.20)

Build clean, 66 tests passing. Output byte-identical to the previous build.

## M4b-3: `--dry-run` and the naming census (v0.1.22, 2026-10-02)

### Build and regression

- Built clean; **117 tests pass**; committed and tagged `v0.1.22`.
- `verify-split.ps1` (the previous release, v0.1.19, against the new build, on `tsp-tester.pst` and the `.msg` fixtures):

| Output | Result | Size |
|---|---|---|
| default | IDENTICAL | 850 bytes, 33 lines |
| `--verify` | IDENTICAL | 2705 bytes, 82 lines |
| PST diagnostic | IDENTICAL | 1092 bytes, 44 lines |

  Standard error was empty in all three.
- The new `--verify` run over the 29 `.msg` fixtures (`verify3.txt`) reports the same key values as before: `files_scanned=29`, `rtf_decompressed_bytes_mismatch=1` (delta −1, the documented dictionary divergence), `embedded_message_class_readable_total=1`, `structural_gate_violations=0`, `total_entries=2647`, `opaque_payload_entries_total=6`, every other comparable field matching.
- The owner also fixed `root_stem` in `dry_run.rs` to work on Windows paths.

### The census

Run with a deliberately very long `--out` (an absolute output path of 141 units, so the roots were 146 and 152) so that the budget gates are exercised.

| Key | `.msg` directory (recursive) | `tsp-tester.pst` |
|---|---|---|
| `plan_root_units` | 146 | 152 |
| `plan_entries_total` | 74 | 154 |
| `plan_folders` | 3 | 10 |
| `plan_messages` | 34 | 60 |
| `plan_attachment_files` | 34 | 84 |
| `plan_embedded_messages` | 3 | 0 |
| `plan_names_sanitized` | 7 | 16 |
| `plan_names_truncated` | 7 | 34 |
| `plan_names_fallback` | 0 | 3 |
| `plan_reserved_name_hits` | 0 | 0 |
| `plan_collision_groups` (largest) | 0 (0) | 3 (16) |
| `plan_budget_exceeded` | 0 | 4 |
| `plan_max_path_units` (allowed 259) | 253 | 266 |
| `plan_max_relative_path_units` | 107 | 114 |
| `plan_max_component_units` | 92 | 69 |
| `plan_max_depth` | 6 | 6 |
| `plan_max_entries_in_directory` | 33 | 22 |
| `plan_gate_violations` | 0 | 4 |
| `source_subdirectories_mirrored` | 3 | 0 |
| `source_subject_markers_stripped` | 0 | 60 |
| `source_attachments_without_name` | 0 | 3 |
| `source_internet_message_id_duplicate` | 4 | 20 |
| `source_embedded_attachments_not_opened` | 0 | 3 |
| `source_attachment_tables_with_long_name_column` | 0 | 9 |
| `folder_identity_nid_available` / `_entry_id_available` / `_unavailable` | 0 / 0 / 0 | 11 / 11 / 0 |

All other keys (`source_open_errors`, `source_non_mail_items`, `source_associated_items_skipped`, read-error counters, the other gate counters) were 0 in both runs.

### What the numbers show

- **Internal consistency.** For both inputs, root units plus the longest relative path equals the longest path (146 + 107 = 253; 152 + 114 = 266). The root differs by 6 units because `tsp-tester` is six characters longer than `msgs`.
- **PST over-budget entries are the expected effect of the long output path.** At a root of 152 units, four entries cannot fit in 259 even at the planner's name floors, and the gate reports exactly those four (`plan_budget_exceeded=4`, `plan_gate_over_budget=4`, `plan_gate_violations=4`). The `.msg` run, with a root of 146 and a longest relative path of 107, fits (253 ≤ 259). The planner therefore reports a problem rather than hiding it; M4c's export refuses to run on such a plan. **Not yet run:** a PST dry run with a short `--out`, which should show 0 violations. (For the `.msg` directory, the M4c export below ran at a short `--out` with 0 violations.)
- **Nameless attachments match the embedded messages the PST side cannot open.** `source_attachments_without_name`, `source_embedded_attachments_not_opened`, and `plan_names_fallback` are all 3 on the PST, and the planner names those entries by fallback.
- **Collision handling works on real names.** The PST has 3 collision groups (largest 16) and `plan_gate_collisions=0`.
- **Folder identity is available on the PST.** Both a node ID and an entry ID were readable for all 11 folders walked, none unavailable. The planned folder count is 10 and 11 were examined, which is consistent with one folder (the root) being identified but not exported; this has not been confirmed separately.
- **Duplicate Internet message IDs are common in the fixtures** (4 of 34 messages in the `.msg` directory, 20 of 60 in the PST), so an Internet message ID cannot be used as a unique key.

### Open: counts that differ from what was predicted

The M4b-3 plan predicted, from earlier diagnostics, 9 planned folders, 57 messages, and 79 attachments for the PST, and 29 messages, 1 embedded message, and 29 attachments for the `.msg` directory. The census reports 10 / 60 / 84 and 34 / 3 / 34.

- For the `.msg` directory the likely explanation is scope: the earlier counts covered the 29 top-level files, while the dry run plans recursively and mirrors 3 subdirectories. The split of the extra 5 messages and 2 embedded messages across those subdirectories has not been checked.
- For the PST there is **no confirmed explanation**: +1 folder, +3 messages, +5 attachment files. Either the fixture changed after the earlier diagnostic, or the planner walks something the diagnostic does not. The current PST diagnostic output (`tsp tsp-tester.pst`) is needed to settle it, and the PST export stage (M4i) is gated on the archive reconciling with the diagnostic.

## M4c: walking skeleton (complete 2026-10-03; tag `v0.1.23.1`, `Cargo.toml` version `0.1.23`)

### Build and regression

- Built clean; **155 tests pass** (117 existing plus 38 new); committed and tagged `v0.1.23.1`. Added dependencies: `serde`, `serde_json`, `sha2`.
- `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` reported no errors, and the CI workflow (Linux and Windows) succeeded on the pushed commit.
- `verify-split.ps1` again against v0.1.19: default, `--verify`, and PST outputs **IDENTICAL**, with the same sizes as before (850 / 2705 / 1092 bytes; 33 / 82 / 44 lines; stderr 0).
- The `--verify` run over the 29 `.msg` fixtures (`verify4.txt`) has the same key values as `verify3.txt`.
- Both `--dry-run` censuses, run again with the same very long `--out`, are **identical to the v0.1.22 census** in every key (the PST still reports 4 over-budget entries at that path). Adding the export path therefore changed neither the planner's results nor the diagnostics.

### What the new tests cover

Synthetic `.msg` files built at test time, no personal data. The two golden files (`tests/golden/hello/message.md`, `metadata.json`) are compared byte for byte, both against the pure renderer and against the files an export writes. Other tests cover: the exact output layout; two exports to different directories being byte-identical; a second identical export writing nothing and needing no consent; a directory input mirroring subdirectories and numbering duplicate subjects; a trailing separator on the input not renaming the archive; unusable characters in a subject being replaced and recorded; attachments being counted and flagged, with no `attachments/` directory written; a message with no plain-text body; the consent matrix (changed files refused without consent, declined at the prompt, accepted at the prompt, replaced with `--overwrite`); unrelated files being left alone; a directory `tsp` did not create being refused even with `--overwrite`; an empty existing target being accepted; an archive of a different source being refused; an over-budget plan writing nothing; an over-long `--out` being an error; a leftover staged file from an interrupted run being cleaned up; a `.pst` export being refused as not implemented; and the command-line flag rules. One test (over-budget) failed on the first run because of an arithmetic error in the test's own setup, was corrected, and all others passed on the first run.

### First export of the real `.msg` corpus (2026-10-03)

`tsp "<fixtures>\msgs" --out C:\o` (the directory of 29 top-level `.msg` files plus 3 subdirectories), then the same command again, then the same input to `C:\o2`. Output directories were created beforehand and empty.

| Key | Run 1 (`C:\o`) | Run 2 (`C:\o`, repeat) | Run 3 (`C:\o2`) |
|---|---|---|---|
| `export_root_units` | 9 | 9 | 10 |
| `plan_entries_total` / folders / messages | 74 / 3 / 34 | same | same |
| `plan_attachment_files` / `plan_embedded_messages` | 34 / 3 | same | same |
| `plan_budget_exceeded` / `plan_gate_violations` | 0 / 0 | 0 / 0 | 0 / 0 |
| `plan_max_relative_path_units` | 181 | 181 | 181 |
| `export_target_state` | absent | owned | absent |
| `export_preflight_to_create` / identical / to_replace / blocked / unrelated | 72 / 0 / 0 / 0 / 0 | 0 / 72 / 0 / 0 / 0 | 72 / 0 / 0 / 0 / 0 |
| `export_consent` | not_needed | not_needed | not_needed |
| `export_result` | completed | completed | completed |
| `export_files_written` / unchanged | 72 / 0 | 0 / 72 | 72 / 0 |
| `export_folder_files` / `export_message_directories` | 4 / 34 | 4 / 34 | 4 / 34 |
| `export_attachments_not_extracted` / `export_embedded_messages_not_extracted` | 34 / 3 | 34 / 3 | 34 / 3 |
| `export_bodies_plain_present` / empty / absent | 34 / 0 / 0 | 34 / 0 / 0 | 34 / 0 / 0 |
| `export_readback_mismatches` | 0 | 0 | 0 |
| `export_tree_sha256` | `96da290e…793ad` | identical | identical |

What this shows:
- **The counts reconcile with the dry run.** 72 files = 4 `folder.json` (the root and 3 mirrored subdirectories) + 34 messages × 2; 74 planned entries = 3 + 34 + 34 + 3. Every one of the 34 messages has a plain-text body.
- **The budget gate passes at a short `--out`.** At a root of 9 units there are 0 violations and the longest planned relative path is 181 units (it was 107 at the long root, because names were shortened more there). This closes the "short `--out`" item for the `.msg` directory; the PST has not been run at a short `--out`.
- **A repeat export is a no-op.** The second run finds the target `owned`, all 72 files identical, writes nothing, and needs no consent.
- **The result does not depend on where it is written.** The tree hash is the same for `C:\o` and `C:\o2` (roots of 9 and 10 units). The hash covers relative paths and file contents as rendered in memory; the read-back count (0 mismatches) shows the files on disk match what was rendered.

### Manual checks on the exported tree (2026-10-03)

- **Tree shape.** `C:\o` held 72 files, no `.tsp-tmp` directory remained, and the longest file path was 131 units (the plan's worst case was 190, because the plan reserves room for attachment files that M4c does not write yet).
- **Content review.** The owner opened one exported `message.md` and its `metadata.json` and found the contents as expected. One message of 34 was reviewed; the rest were not.
- **Interactive overwrite on real output.** After a line was appended to one exported `message.md`, re-running in an interactive terminal showed the prompt (1 file to replace, 71 identical, 0 unrelated), the owner answered yes, and the run completed: `export_consent=prompted_accepted`, `export_files_written=2` (the restored file and the root `folder.json`, which is rewritten whenever anything changes), `export_files_unchanged=70`, exit code 0, and the same tree hash as before the edit, so the edited file was restored byte for byte. The next two runs (one with `--overwrite`, one plain) found all 72 files identical and wrote nothing (`export_consent=not_needed`, since there was nothing left to replace).
- **Non-interactive refusal and `--overwrite` on real output.** After appending a line to one exported `message.md` again, running with standard input redirected (`$null | tsp.exe ...`) printed "1 existing file(s) would be replaced; run interactively or add --overwrite; nothing was written", reported `export_preflight_to_replace=1`, `export_consent=non_interactive_declined`, `export_result=refused_needs_consent`, `export_files_written=0`, and exited with code 2. Re-running with `--overwrite` reported `export_consent=overwrite_flag`, `export_result=completed`, `export_files_written=2`, `export_files_unchanged=70`, and the same tree hash as every earlier run, so the file was restored. Because the replaced file already existed, this also shows that renaming over an existing file works on the owner's Windows setup.

### Not yet run

- A PST dry run with a short `--out`, and the current PST diagnostic output (both still open from M4b-3).
- The remaining 33 exported messages have not been individually reviewed, and the corpus is a single producer.

### Status

**M4c is complete.** It is built, formatted, lint-clean, and CI-green; its 155 tests pass; it exported the real `.msg` corpus with counts that reconcile and a stable tree hash; the owner reviewed an exported message; and the interactive prompt, the non-interactive refusal (exit code 2), and `--overwrite` all behaved as designed on real output. The M4a-2 decision records and the full M4b-4 model are not done, so the `0.1-draft` schemas are not frozen. The housekeeping items remain: `#![allow(dead_code)]` in `naming.rs` and `plan.rs`, and the `0.1.23` Cargo version against the `v0.1.23.1` tag.

## M4a-2: the four ADRs (accepted 2026-10-04; tag `v0.1.24`, documentation only)

No code changed and nothing was run. The owner read and accepted the four ADRs (naming, collisions, and path budgets; identity and provenance; output posture, consent, and ownership; archive file contract and schema evolution). The body and formatting-loss ADR is not drafted because it depends on the HTML-to-Markdown comparison (M4e).

## M4d: envelope (code at `v0.1.25`; tag `v0.1.25.1`, 2026-10-06)

What was built: the envelope (sender, sent-representing, To/Cc/Bcc recipients, submit and delivery times, importance, sensitivity, conversation topic and index, transport headers) is read by the custom MS-OXMSG path (`oxmsg_envelope.rs`), held in the model (`Envelope`, `Address`, `Recipient`), shown as a header list in `message.md`, and recorded in an `envelope` object in `metadata.json` (draft schema). The status reason `envelope_not_extracted` became `other_properties_not_preserved`. A new `--verify-envelope` mode compares the subject, sender, and recipients with `msg_parser`. Property identifiers and how well each is confirmed: [`../plans/m4d-envelope-properties.md`](../plans/m4d-envelope-properties.md).

### Build and regression (2026-10-06)

- The owner reports that everything built and **165 tests pass** (155 existing plus 10 new); the work is committed on GitHub and tagged `v0.1.25.1`. The `v0.1.25` commit says the code was written uncompiled; `v0.1.25.1` adds the committed golden file `tests/golden/envelope/message.md` (an envelope-rich synthetic message, rendered by the pure renderer and compared byte for byte). The `hello` golden files changed with the new header list and status reason.
- **Not reported for this round:** `cargo fmt --check`, `cargo clippy -D warnings`, and CI results, and a real-corpus export (so there is no `export_tree_sha256` for the new format; it changes with every envelope field, so the earlier `96da290e…793ad` no longer applies).
- `verify-split.ps1` (`verify-split5.txt`), the pre-split baseline against the new build, on `tsp-tester.pst` and the 29-file `msgs` directory:

| Output | Result | Size |
|---|---|---|
| default | IDENTICAL | 850 bytes, 33 lines |
| `--verify` | IDENTICAL | 2705 bytes, 82 lines |
| PST diagnostic | IDENTICAL | 1092 bytes, 44 lines |

  Standard error was empty in all three. The key lines of the new `--verify` output are unchanged: `files_scanned=29`, `rtf_decompressed_bytes_mismatch=1`, `embedded_message_class_readable_total=1`, `structural_gate_violations=0`. Adding the envelope therefore changed none of the existing diagnostics. (The script's cleanup line names its baseline worktree `teaspoon_v0.1.19.1`; earlier results call the baseline v0.1.19.)

### `--verify-envelope` over the 29 top-level `.msg` files (`verify-envelope1.txt`)

`tsp --verify-envelope <fixtures>\msgs`. The report keys are counts only. `subdirectories_skipped=3`: the scan is non-recursive, so the 5 further messages in the 3 subdirectories (which the export includes) were not compared.

| Field compared with `msg_parser` | Matched | Mismatched |
|---|---|---|
| subject | 29 | 0 |
| sender name | 29 | 0 |
| To list (length) / Cc list / Bcc list | 29 / 29 / 29 | 0 / 0 / 0 |
| recipient name (36 recipients across the three lists) | 36 | 0 |
| sender email | 29 (see below) | 0 |
| recipient email | 36 (see below) | 0 |

`open_errors_msg_parser=0` and `open_errors_custom=0`.

Which of the custom path's two email properties equalled `msg_parser`'s single email string (an absent property is compared as an empty string, and "both" includes the case where both are empty):

| | both | `PidTagEmailAddress` only | SMTP property only | neither |
|---|---|---|---|---|
| sender (29) | 1 | 26 | 2 | 0 |
| recipients (36) | 0 | 33 | 3 | 0 |

Presence counts, with nothing in `msg_parser` to compare against: sender 29 (`sender_missing_total=0`), sent-representing 29, submit time 29, delivery time 29, importance 29, sensitivity 11, transport headers 24, conversation topic 29, conversation index 29. `recipients_unlisted_total=0` and `exchange_without_smtp_total=0`.

What this shows:
- **No disagreement with the oracle on any compared field**, over 29 messages and 36 recipients. The differential gate for the comparable fields is met on this corpus.
- **The email property question is partly answered.** `msg_parser`'s email string is not always `PidTagEmailAddress`: in 5 of 65 comparisons (2 senders, 3 recipients) it equals the SMTP property and not `PidTagEmailAddress`; in the other 60 it equals `PidTagEmailAddress`. That fits the reading that `msg_parser` prefers an SMTP value when one exists, but the counts cannot show it: `email_address_only` does not separate "the SMTP property is absent" from "present and different". Separating the two would settle whether `message.md`, which shows the SMTP address first, ever disagrees with `msg_parser`.
- **The SMTP identifiers gained real-data evidence.** The SMTP-only matches (2 senders, 3 recipients) show that the properties read as the sender and recipient SMTP addresses (0x5D01, 0x39FE) carry the values `msg_parser` reports. The same holds for the sender name (0x0C1A), sender address (0x0C1F), recipient name (0x3001), and recipient address (0x3003) on every message and recipient.
- **Some paths still have no real instance.** No message has an `EX` address without an SMTP address, no recipient row falls outside To/Cc/Bcc, and none is unlisted. Those paths are covered by unit tests only.
- **Fields without an oracle** (sent-representing, times, importance, sensitivity, conversation fields, transport headers) are shown to be present in the corpus (above), but their values were not checked against anything independent. Sent-representing is present on all 29 messages.

### Manual check

The owner examined a couple of exported `message.md` and `metadata.json` pairs and found them as expected. Which messages, and how many, were not recorded.

### Status

**M4d is built and its differential gate is met for the fields `msg_parser` exposes** (0 mismatches), existing outputs are unchanged, and 165 tests pass. It is **not closed against the plan's own requirement** that every property identifier be confirmed against Microsoft's specifications: tier B and C identifiers remain, and the fields without an oracle rest on the specification and the unit tests. Open items:

- Confirm the remaining identifiers (0x0064, 0x0065, 0x3002, the importance and sensitivity meanings, and the others listed as tier B or C) against MS-OXPROPS.
- Separate "SMTP property absent" from "present and different" in the email tally.
- Compare the 5 messages in the subdirectories (the scan is non-recursive).
- ~~Run an export of the corpus to record the new `export_tree_sha256`~~ (done 2026-10-07, below). `cargo fmt --check`, `clippy`, and CI for the tags were later reported successful (below).
- Review more exported messages, and a second producer, before treating the envelope as proven beyond one corpus.

### M4d loose ends (2026-10-07; tags `v0.1.25.1` and `v0.1.25.2`)

- **New corpus baseline.** The owner reports that the export of the `.msg` corpus at `v0.1.25.1` (the same at `v0.1.25.2`, the documentation-only tag after the M4d documents were applied) reports `export_tree_sha256=72d8553417441719c7c1e2b1d3ec253141fb5c2697c311d4210a92cd5d71c2e5`. This replaces `96da290e…793ad` (v0.1.23.1), which no longer applies because the output format changed. The other `export_*` counts for this run were not reported.
- **PST short-`--out` dry run (2026-10-07), closed.** `tsp.exe tsp-tester.pst --dry-run --out $o` with `plan_root_units=15`: `plan_entries_total=154`, folders 10, messages 60, attachment files 84, embedded 0, names sanitized 16, truncated 2, fallback 3, collision groups 2 (largest 16), `plan_budget_exceeded=0`, `plan_max_path_units=253` (allowed 259), `plan_max_relative_path_units=238` (15 + 238 = 253), `plan_max_component_units=115`, depth 6, `plan_gate_violations=0` (all four gates 0). So the 4 over-budget entries seen at a root of 152 units fit at a root of 15; the PST plan is clean at a short output path. The other keys equal the long-path census (subject markers 60, nameless attachments 3, duplicate Internet message IDs 20, embedded not opened 3, long-name columns 9, folder identity 11/11/0). The PST count difference from the earlier diagnostic (60/84 against 57/79) is still **unreconciled**: the current `tsp tsp-tester.pst` diagnostic output has not been supplied.
- **Earlier, without `--dry-run`:** Running `tsp.exe tsp-tester.pst --out $o` without `--dry-run` stops with "exporting a .pst file is not implemented yet (planned for M4i); add --dry-run to plan one". That is the designed refusal and not a failure; the open item needs `--dry-run` with a short `--out`.
- **Identifiers.** Every tier B and C property identifier was checked on 2026-10-07 against Microsoft's pages (the MAPI canonical property pages; MS-OXPROPS for 0x0C1A) and is now tier A: 0x0064, 0x0065, 0x0C1A, 0x0C15, 0x0E06, 0x3001, 0x3002, 0x3003, 0x39FE, and the identifiers for importance (0x0017) and sensitivity (0x0036). Sensitivity values 0 to 3 and importance high = 2 were also seen in Microsoft text; importance low = 0 and normal = 1 were not. **A finding:** Microsoft's PidTagRecipientType page says the value is one type plus an optional flag (MAPI_P1, MAPI_SUBMITTED), while the export lists only values 1, 2, 3, so a flagged recipient would be dropped from `message.md` (counted in `recipients_unlisted`); the corpus has none (`recipients_unlisted_total=0`), so the differential run could not catch it. See [`../plans/m4d-envelope-properties.md`](../plans/m4d-envelope-properties.md).
- **Built, not yet run (v0.1.26 candidate):** the email tally now separates "SMTP absent" from "SMTP present and different", and `--recursive` lets `--verify-envelope` cover the 5 messages in the subdirectories. Both need one run to produce numbers.
- **`cargo fmt --check`, `clippy -D warnings`, and CI** were all successful for the tags 0.1.25, 0.1.25.1, and 0.1.25.2 (owner's report; the third tag was typed `0.1.25.22` and is read as `0.1.25.2`).

## M4e-1 and the M4d follow-ups: built and tagged `v0.1.26` (2026-10-08)

### Build and regression

- The owner reports that everything built, tested, and passed CI after he addressed several build issues and upgraded the crate to **Rust edition 2024** (with all dependencies updated, in the same tag); **all 201 tests pass** (165 plus 36 new); the repository is tagged `v0.1.26`.
- Not reported for this tag: `verify-split.ps1` (the default, `--verify`, and PST outputs against the baseline), and the corpus export tree hash (the output format did not change, so `72d85534…c2e5` should still hold).

### What was written and how it was revised

`src/rtf_deencap.rs` (the recognition rule, an RTF tokenizer, and the MS-OXRTFEX extraction rules, with diagnostics and bounds), `src/verify_deencap.rs` (`--verify-deencap`), edits to `cli.rs`, `main.rs`, and `verify_envelope.rs`. Review of the owner's revisions to four files:

| File | Revision | Assessment |
|---|---|---|
| `rtf_deencap.rs` | A `TrailingBackslash` token replaces the drafted `Symbol(b'\\')` for a backslash at the very end of input; the interpreter ignores it. A second test (`a_trailing_backslash_is_dropped_but_a_escaped_one_is_not`) was added. Let-chains in two places; edition-2024 import order and `rustfmt` layout. | The first is a real fix: the drafted code wrote a literal backslash for a lone trailing backslash, which its own test (`a_trailing_backslash_is_harmless`) would have caught. Semantics otherwise identical. The older test is now partly redundant with the new one. |
| `verify_deencap.rs` | New `recognition_mismatch_by_kind` map and `recognition_mismatch_kind_<kind>_files` lines (which kind the 10-token rule concluded when it disagreed with the marker search); edition-2024 imports and layout. | A useful addition: it says what the disagreeing files are. **Slip:** the `recognition_vs_marker_search` tally is now printed twice (once before and once after `html_presence`), so every `recognition_vs_marker_search_*` line appears twice in the output. Harmless to the numbers, but the second call should be deleted. |
| `verify_envelope.rs` | Edition-2024 import order only. | Identical to the drafted version in behavior. |
| `export.rs` | Import order; one nested `if` and `if let` collapsed into a let-chain in `relative_source_name`; `bail!` and `assert!` arguments re-wrapped. | No behavior change: when the path is not under the input the function still falls through to the file name. |

### A defect found in the review (not in the four files): `source_pst.rs`

At `v0.1.26` the loop over a folder's sub-folders in `PstBuilder::folder` appears **twice** in `source_pst.rs` (two identical `if depth < MAX_FOLDER_DEPTH && let Some(hierarchy) = folder.hierarchy_table() { ... }` blocks, apparently from the edition migration). Each sub-folder is opened and pushed twice, and each of those copies repeats the doubling for its own sub-folders, so a folder at depth `d` is planned `2^d` times; the folder identity counts double as well. The 201 tests do not cover it (no PST fixture is in the repository), and no PST dry run was reported at `v0.1.26`; the one reported on 2026-10-07 was at `v0.1.25.x`, before the migration. The default PST diagnostic (`pst.rs`) and `--verify` are unaffected. Fix: delete the second block. Check: the PST dry run with a short `--out` should again report `plan_folders=10`, `plan_messages=60`, `plan_attachment_files=84`, `plan_entries_total=154`, and `folder_identity_nid_available=11`. **Fixed in `v0.1.26.2` and confirmed** (below).

### Gate for M4e-1 (state at `v0.1.26.2`; the corpus run is below)

| Gate item | State |
|---|---|
| Spec-derived unit tests, and the property tests (arbitrary input never panics, output bounded) | **met**: part of the 203 passing tests |
| Recognition rule against `check_compressed_rtf_bytes` | **met on this corpus**: 0 disagreements in 29 files |
| Weak-oracle differential against `msg_parser`'s `html_from_rtf()` on the 27 encapsulated-HTML messages, counts only, every disagreement triaged | **not met**: the presence agrees on all 29 files, but the recovered HTML differs from `msg_parser`'s in all 27 pairs at the coarsest grade, and no disagreement has been triaged yet |
| Hand comparison of a sample against a reference implementation | not done |

### Identifiers and recipient types (2026-10-07)

Every property identifier in the M4d envelope was checked against Microsoft's pages (the MAPI canonical property pages, and MS-OXPROPS for 0x0C1A) and is tier A. Sensitivity values 0 to 3 and importance high = 2 were seen in Microsoft text; importance low = 0 and normal = 1 were not. **A likely defect:** Microsoft's PidTagRecipientType page says the value is one type plus an optional flag (MAPI_P1, MAPI_SUBMITTED), but the envelope lists only values 1, 2, 3, so a flagged recipient would be dropped from `message.md` (counted in `recipients_unlisted`). The corpus has none (`recipients_unlisted_total=0`), so the differential run could not catch it. Details: [`../plans/m4d-envelope-properties.md`](../plans/m4d-envelope-properties.md).

## `v0.1.26.2`: corrections applied, and the first corpus runs of the new modes (2026-10-08)

### Build and corrections

The owner applied three corrected files (the duplicated sub-folder loop removed from `source_pst.rs`; the duplicated recognition tally print removed from `verify_deencap.rs`, checked: each `recognition_vs_marker_search_*` line now appears once; the recipient-type flags MAPI_P1 and MAPI_SUBMITTED ignored when classifying in `oxmsg_envelope.rs`, with two tests). Everything built, **203 tests pass** (201 plus 2), committed, and tagged `v0.1.26.2`.

### PST dry run with a short `--out` at `v0.1.26.2` (`plan_root_units=20`)

`plan_entries_total=154`, folders 10, messages 60, attachment files 84, embedded 0, names sanitized 16, truncated 2, fallback 3, collision groups 2 (largest 16), `plan_budget_exceeded=0`, `plan_max_path_units=253`, `plan_max_relative_path_units=233` (20 + 233 = 253), `plan_max_component_units=115`, depth 6, `plan_gate_violations=0`; folder identity 11 / 11 / 0; the `source_*` keys unchanged. These equal the counts before the migration (root 15 on 2026-10-07), so **the duplicate-folder defect is fixed** and the migration changed no PST plan count. The relative path is 5 units shorter than at root 15 only because the root is 5 units longer (budget arithmetic). The PST count difference from the earlier diagnostic (60/84 against 57/79) is still unreconciled.

### `--verify-envelope --recursive` over the corpus (34 files, `verify-envelope-recursive1.txt`)

0 open errors; the scan now includes the 5 messages in the 3 subdirectories (`subdirectories_skipped=0`).

| Field compared with `msg_parser` | Matched | Mismatched |
|---|---|---|
| subject, sender name | 34, 34 | 0, 0 |
| To / Cc / Bcc list length | 34 / 34 / 34 | 0 / 0 / 0 |
| recipient name (41 recipients) | 41 | 0 |
| sender email (34) | 34 | 0 |
| recipient email (41) | 41 | 0 |

| Email property that equalled `msg_parser` | both | `PidTagEmailAddress` only | SMTP only |
|---|---|---|---|
| sender (34) | 1 (0 of them both empty) | 31, all with the SMTP property absent, 0 with it present and different | 2, both with `PidTagEmailAddress` present and different |
| recipients (41) | 0 | 38, all with the SMTP property absent, 0 with it present and different | 3, all with `PidTagEmailAddress` present and different |

Presence counts (of 34): sender 34, sent-representing 34, submit time 34, delivery time 34, importance 34, sensitivity 11, transport headers 29, conversation topic 34, conversation index 34; `recipients_unlisted_total=0`, `exchange_without_smtp_total=0`.

What this shows:
- **The M4d differential gate is met on all 34 messages** (the 5 subdirectory messages added 5 recipients and 0 mismatches).
- **The email question is answered for this corpus.** In every one of the 5 cases where both of the custom path's email properties are present and differ (2 senders, 3 recipients), `msg_parser` returned the SMTP address; in every other case the SMTP property was absent and `msg_parser` returned `PidTagEmailAddress`. That is exactly the rule `message.md` uses (the SMTP address, else the address when its type is SMTP or absent). There is no case where the SMTP property is present, differs, and `msg_parser` chose the other one, so on this corpus `message.md` never disagrees with `msg_parser`.
- Still no real instance of an `EX` address without an SMTP address, an unlisted recipient, or a flagged recipient type.

### `--verify-deencap` over the 29 top-level files (`verify-deencap1.txt`)

0 open errors; `subdirectories_skipped=3` (this run was not recursive).

| Measure | Result |
|---|---|
| RTF kind by the 10-token rule | encapsulated HTML 27; encapsulated text 1; plain RTF 1; not RTF 0; RTF absent 0; decompression failed 0 |
| Recognition rule against the whole-document `\fromhtml1` search | both true 27, both false 2, **mismatch 0** |
| HTML recovered, custom against `msg_parser` | both true 27, both false 2, **mismatch 0** |
| Agreement of the 27 recovered HTML pairs | exact 0; equal ignoring whitespace 0; ignoring whitespace and non-ASCII 0; same visible text 0; same visible text ASCII only 0; **different 27** |
| Size, custom minus `msg_parser`, per pair | from −1571 to +1586 bytes (three pairs at +15); totals 1,069,793 against 1,067,224 bytes (custom 0.24% larger) |

Diagnostics: 15,024 `htmltag` groups; 491 ignorable and 27 standard destinations skipped; 13,446 `\htmlrtf` regions; 677 hex escapes and 429 `\uN` escapes (0 bad hex escapes, 0 unpaired surrogates); 8,561 paragraph breaks and 977 tabs; 122 fonts defined; 0 control symbols ignored, 0 binary bytes, 0 object placeholders, 0 stray group ends; 0 files with unclosed groups, with the depth limit hit, with the output limit hit, or with a `charset=` declaration in the recovered HTML; default code page 1252 declared in 27 files; **1 undecodable byte in 1 file, with code page 1255 unsupported**.

What this shows, and what it does not:
- **Recognition is settled for this corpus:** the MS-OXRTFEX 10-token rule and the existing whole-document search give the same answer on all 29 files, so the M4e-1 item "confirm the recognition rule" is met. The two non-HTML files are one `\fromtext` message and one plain-RTF message.
- **Two hypotheses in the earlier draft were wrong and are withdrawn:** there is no presence disagreement for the RTF-authored message (`msg_parser` returns no HTML for it here; the dependencies were updated in `v0.1.26`, which may explain the change from M2, but that was not checked), and the 1252-only decoder is not a problem for nearly the whole corpus.
- **The content of the 27 recovered HTML bodies differs from `msg_parser`'s in every file, at every grade, including the visible text outside tags with whitespace and non-ASCII characters removed.** The totals are within 0.24%, the deltas run both ways, and three pairs differ by exactly 15 bytes, so there is no single large systematic cause, but counts alone cannot say which side is right or why. Possible causes, none of them checked: treatment of text inside `\htmlrtf` regions, of `\uN` fallbacks, of `\par` and `\tab`, of ignorable destinations, of entities, or of the `\htmltag` content boundary. This is the open item; the differential gate stays unmet until the differences are triaged against the specification.
- **Code pages:** the only unsupported one is 1255 (Hebrew), for 1 byte in 1 of 27 files. That does not justify a code page crate on its own; the corpus is one producer.
- **No file declares a `charset=`** in its recovered HTML, so the stale-declaration concern did not arise here.

## `--dump-deencap` and the triage of the 27 HTML differences (2026-10-09)

### The run

`--verify-deencap --dump-deencap C:\dump1` over the 29 top-level files: the report is identical to the earlier run plus `dump_files_written=83` (27 messages × 3 files, plus the decompressed RTF alone for the 2 messages that are not encapsulated HTML). Everything built and all **209 tests** passed (203 plus 6 for the dump). The owner also re-ran `--verify-envelope --recursive` (identical to the first run, 0 mismatches) and a PST dry run with a short `--out` at a root of 18 units (identical counts: 154 entries, 10 folders, 60 messages, 84 attachment files, 0 gate violations; relative path 235, 18 + 235 = 253).

### What the pairs show

The dumped HTML and RTF were compared directly (the content was read on the assistant's side, from files the owner supplied). `msg_parser`'s output differs from the RTF's own content in six ways, and the in-house output differs from `msg_parser`'s only because of them, with one exception:

| # | Difference | Which side is right |
|---|---|---|
| 1 | `msg_parser` prepends the font table's text (for example `Arial;Courier New;Symbol;...;*`), 40 to 74 bytes per file | in-house; the font table is not HTML |
| 2 | `msg_parser` inserts a `*` after each encapsulated tag group (5 to about 3,900 per file) | in-house |
| 3 | `msg_parser` drops every line break; the in-house output keeps the `\par` and line breaks the encapsulation recorded (as CRLF) | not settled: no oracle keeps them; the in-house rule follows MS-OXRTFEX, and the breaks match the original HTML's line structure in the pairs inspected |
| 4 | `msg_parser` reads `\'hh` bytes as Latin-1, so a Windows-1252 byte such as 0x93 becomes a C1 control character; the in-house output decodes it in the font's code page (a curly quote) | in-house |
| 5 | Where `\uN` has a fallback character, `msg_parser` emits the fallback (`?`, or the Latin-1 byte), not the Unicode character: bullets from the Symbol font become `?`, an emoji becomes `???`, a narrow no-break space becomes a no-break space | in-house (the fallback is for readers without Unicode) |
| 6 | In one file `msg_parser` corrupts a word (`about` becomes `obout`); the RTF contains `about`. Only this file's letters-and-digits stream differs between the two outputs, and the cause is consistent with the dictionary divergence recorded in M3 (a wrong preset dictionary gives wrong bytes) | in-house (via `compressed-rtf`, which matches MS-OXRTFCP) |
| 7 | **In-house limitation:** one byte (`\'fe` in a font of code page 1255, Hebrew) became U+FFFD because Windows-1255 is not implemented; the correct character is U+200F (right-to-left mark), and `msg_parser` shows `þ` (Latin-1), also wrong | neither; known and counted (`deencap_undecodable_bytes_total=1`) |

After removing 1 (the prefix), 2 (the stars), and whitespace, and mapping the in-house characters to `msg_parser`'s Latin-1 and `?` style, **24 of 27 pairs are identical**; the three that remain are explained by 5 (two files) and 6 (one file), and by 7 for the one unrepresentable byte. No difference was found that points to an in-house defect other than 7.

### Status of the M4e-1 gate

| Gate item | State |
|---|---|
| Spec-derived unit tests and the property tests | met (209 tests) |
| Recognition rule against `check_compressed_rtf_bytes` | met on this corpus (0 disagreements in 29 files) |
| Weak-oracle differential against `msg_parser`, every disagreement triaged | **met**: all 27 differing pairs are explained (above) |
| Hand comparison of a sample against a reference implementation | not done; the line-break rule (3) in particular has no independent oracle |

Residual, not blocking: Windows-1255 and the other multi-byte or unimplemented code pages (1 byte in 1 of 27 files here, one producer); line-break placement unverified against a second implementation; `\fromtext` encapsulated plain text not handled (1 file; the plain-text body exists anyway); the decompressed RTF still passes through `compressed-rtf`'s string result.

### Which of the files supplied were useful

`dump1.zip` was the decisive one. The two re-run reports (`verify-envelope-recursive2.txt`, `pst-dry-run1.txt`) repeated earlier results exactly, and `verify-deencap2.txt` added only the `dump_files_written=83` line. Still outstanding from the earlier requests: the current `tsp tsp-tester.pst` diagnostic output, `verify-split.ps1` at the current tag, and a corpus export tree hash at the current tag.

## M4e-0: HTML-to-Markdown bake-off (first run 2026-10-10)

### Build and cost

Everything built and **225 tests pass** (209 plus 16 for the harness; `Cargo.toml` is at `0.1.28`). `cargo tree --features bakeoff --depth 1` shows the three candidates (`htmd` 0.5.4, `html-to-markdown-rs` 3.11.0, `html2markdown` 0.2.0) added to the direct dependencies. Release build time: 47.60 s without the feature, 54.11 s with it (+6.5 s, about 14%). Transitive weight, duplicate versions, and licenses were not reported.

### The run

`--bakeoff-md --bakeoff-dump C:\md1 C:\dump1`: 27 recovered bodies and 16 synthetic cases, four candidates (`htmd` pure, `htmd` faithful, `html2markdown`, `html_to_markdown_rs`), each on the raw HTML and on the lightly preprocessed copy; 360 files dumped. **0 panics, 0 errors, and 0 nondeterministic outputs for every candidate and variant, over all 43 inputs.** Time is not a differentiator (27 bodies in 21 to 79 ms in total for every candidate).

Corpus totals as printed (preprocessed variant; HTML side: 361 links, 140 images, 32 headings, 38 list items, 237 table rows):

| Candidate | output bytes | links | images | headings | list items | table rows | Office noise | raw HTML tags left |
|---|---|---|---|---|---|---|---|---|
| `htmd` | 388,068 | 361 | 140 | 31 | 46 | 0 | 0 | 1 |
| `htmd` faithful | 787,424 | 0 | 0 | 0 | 0 | 0 | 2,246 | 8,052 |
| `html2markdown` | 525,124 | 282 | 73 | 1 | 4 | 34 | 0 | 14 |
| `html-to-markdown-rs` | 254,996 | 345 | 46 | 1 | 322 | 44 | 0 | 15 |

### A flaw in the harness's own word metric (found in review)

The report's `words_missing` (8,577 / 7,851 / 7,631 / 8,759 of 26,952 for the best variants) **is wrong and should be disregarded.** It looked for each HTML word in the next 200 Markdown words after the last match; a common word matched early skipped the position past text that followed, and the very long tracking URLs in the newsletters pushed words out of the window. Recomputed from the dumped pairs of the three largest messages (001, 023: two newsletters; 014: a forwarded web article; 19,344 visible words in all) as a multiset difference with link destinations removed:

| Candidate (preprocessed) | HTML words missing | Markdown words not in the HTML ("extra") |
|---|---|---|
| `htmd` | 2 (0.01%) | 6,384 |
| `htmd` faithful | 2 | 43,015 |
| `html2markdown` | 34 (0.18%) | 53 |
| `html-to-markdown-rs` | 55 (0.28%) | 85 |

So every candidate keeps almost all of the text. The corpus-wide figures need the corrected metric and a re-run; the corrections are described for the next change to `bakeoff.rs` in the plan.

### What the outputs show (the dumped Markdown for messages 001, 014, and 023, read side by side)

- **`htmd` (default mode)** gives the best Markdown by a wide margin: paragraphs separated by blank lines; all 5 and 11 headings of the two newsletters and the 1 of the article kept as `#` headings; all 26, 16, and 11 images and all links kept (including `cid:` images and link titles); simple tables converted, layout tables flattened into readable paragraphs. **Its one serious defect:** it writes the text of `<style>` blocks that sit in the body (the newsletters carry them) into the output, which is the source of its extra words (about 240 braces per newsletter). Removing `<style>` and `<script>` elements before conversion should fix that and is cheap. The single U+FFFD is the Windows-1255 byte from the de-encapsulation step.
- **`htmd` faithful mode** keeps raw HTML in place of Markdown (8,052 raw tags left, no headings or links as Markdown, 1 MB of output; message 014 collapses to 9 lines). Unusable as a body format; rejected.
- **`html2markdown`** runs paragraphs together without a blank line (13 and 32 sentence joins per newsletter against none for `htmd`), leaves numeric entities such as `&#xA;` in the text, puts headings inline after other text instead of on their own lines, keeps 9 of 26 images in message 001, loses 4 of 38 list items' structure, and on the unpreprocessed article loses a large part of the text (12,540 of 13,897 words by the flawed metric, 0 after preprocessing). Rejected.
- **`html-to-markdown-rs`** is nearly identical to `htmd` on the plain article (message 014) and the closest to the HTML's own text, but on the layout-table newsletters it renders layout tables as Markdown tables (about 30 pipe lines with long dash rules per newsletter), loses every heading (0 of 16 in messages 001 and 023), keeps 4 of 26 images in message 001, and drops the hidden preheader sentence that `htmd` and `html2markdown` keep (55 words missing in total). It is also the slowest on the raw variant (79 ms against about 30 ms).
- **Preprocessing matters:** removing Word comments, downlevel markers, and `o:p` tags cut `htmd`'s Office noise from 508 to 0 and `html2markdown`'s from 497 to 0 (and rescued `html2markdown` on message 014). It did not change `html-to-markdown-rs`.

### Preliminary result and what it does not yet show

`htmd` 0.5.4 in its default mode, behind in-house preprocessing that also removes `<style>` and `<script>` elements, is the leading candidate. This is not yet a decision: the three messages examined are the three largest of 27 and come from two sources (a newsletter service and a forwarded web page); the corpus is one producer; the corrected metric has not been re-run over all 27 and the 16 synthetic cases; tables (data tables against layout tables) and hidden text are policy questions the body-policy ADR must settle; and transitive dependency weight and licenses have not been checked. The gate (a recorded choice with the version pinned, in the body-policy ADR) is not met.

## M4e-0: bake-off re-run after the metric fix (2026-10-11)

### Build and cost

The owner reports that everything built and **233 tests pass** (225 plus 8 for the new metric and the style and script removal; the ordered-window test was replaced). `Cargo.toml` is at `0.1.28`; nothing is committed yet, and the owner plans to commit with updated documents and tag `v0.1.29`. Release build time: **45.66 s** without the `bakeoff` feature (47.60 s in the first run, so unchanged within noise) and **1 min 25 s** with it (54.11 s in the first run). The feature therefore adds about 40 s, not the 6.5 s recorded earlier; the earlier figure may have come from an incremental build, which was not checked. `cargo tree --features bakeoff --depth 1` lists the three candidates (`htmd` 0.5.4, `html-to-markdown-rs` 3.11.0, `html2markdown` 0.2.0) beside the direct dependencies (`anyhow` 1.0.104, `cfb` 0.15.0, `clap` 4.6.7, `compressed-rtf` 1.0.1, `msg_parser` 0.3.6, `outlook-pst` 1.2.0, `serde` 1.0.229, `serde_json` 1.0.152, `sha2` 0.11.0, `unicode-normalization` 0.1.25; dev: `proptest` 1.12.0). A depth-1 listing shows no transitive dependency, duplicate version, or license, so **weight and licenses are still not known.**

### What changed in the harness

- **Word metric:** a multiset difference between the HTML's visible words and the Markdown's words (`words_missing`, and the new `words_extra`), after link and image destinations are removed from the Markdown. Order is not compared.
- **Preprocessing ("pre" variant):** now also removes `<style>` and `<script>` elements with their text. Comments and those elements are removed in one left-to-right pass (a `<!--` inside a script is script text; a `<style>` inside a Word conditional comment is comment text); then Word's downlevel markers and `o:p` tags, as before.

### The run

`--bakeoff-md --bakeoff-dump C:\md1 C:\dump1 > bakeoff2.txt` (the first attempt failed with "failed to read the bake-off input directory" because `C:\dump1` did not exist at that path; it was re-run with the directory that holds the `NNN.custom.html` files). 27 recovered bodies and 16 synthetic cases, four candidates, raw and preprocessed variants, 360 files dumped. **0 panics, 0 errors, and 0 nondeterministic outputs for every candidate and variant over all 43 inputs.** Every candidate shows `replacement_chars=1` on the corpus: that is the Windows-1255 byte from the de-encapsulation step, so it comes from the input, not from a converter.

Corpus totals, preprocessed variant (HTML side: 26,952 visible words, 361 links, 140 images, 32 headings, 38 list items, 237 table rows):

| Candidate | words missing | words extra | links | images | headings | list items | table rows | raw tags | noise | bytes |
|---|---|---|---|---|---|---|---|---|---|---|
| `htmd` | 2 | 262 | 361 | 140 | 31 | 46 | 0 | 1 | 0 | 264,071 |
| `htmd` faithful | 0 | 73,542 | 0 | 0 | 0 | 0 | 0 | 8,012 | 2,246 | 653,609 |
| `html2markdown` | 53 | 514 | 282 | 73 | 1 | 4 | 34 | 14 | 0 | 525,124 |
| `html-to-markdown-rs` | 122 | 1,440 | 345 | 46 | 1 | 322 | 44 | 15 | 0 | 254,996 |

Raw variant, for comparison: `htmd` 2 missing, 21,664 extra, 508 noise markers, 414,326 bytes; `htmd` faithful 0 missing, 126,083 extra, 11,748 raw tags, 3,761 noise markers; `html2markdown` 53 missing, 29,510 extra, 2,694 raw tags, 497 noise markers; `html-to-markdown-rs` 111 missing, 1,452 extra, 2 noise markers. Time over the 27 bodies (first run of each, milliseconds): `htmd` 32 raw and 20 preprocessed, `htmd` faithful 38 and 25, `html2markdown` 27 and 21, `html-to-markdown-rs` 76 and 35; not a differentiator.

Synthetic set, preprocessed: `htmd` 1 missing and 3 extra words; `htmd` faithful 1 and 10,684; `html2markdown` 2 and 3; `html-to-markdown-rs` 2 and 3. Every candidate reports `links_md=3` against 2 links in the HTML.

### What the numbers show

- **The `htmd` style-text defect is confirmed and removed by the preprocessing.** On the raw variant `htmd` has 21,664 extra words and 508 noise markers; after preprocessing, 262 and 0, and the output falls from 414,326 to 264,071 bytes. The structure counts (links, images, headings, list items) are the same on both variants. Style and script removal is therefore a requirement of the body pipeline (M4e-2), not an option.
- **The first run's figures are superseded.** The preprocessed `htmd` size recorded in the first run (388,068 bytes) still contained body style text that Word's comment removal had not caught; the figure is now 264,071. The first run's `words_missing` column stays withdrawn.
- **`htmd` default mode keeps almost every word and all the links and images.** 2 of 26,952 words are missing (0.007%), 361 of 361 links and 140 of 140 images are kept, 31 of 32 headings, no Office noise, 1 raw HTML tag. In the first run's hand comparison of the three largest messages (001, 014, 023) `htmd` was also missing 2 words, so these 2 are probably the same ones; the per-input list added to the harness will say which file.
- **The 262 extra words** are plausibly the alt text of the 140 images (the metric counts alt text as extra); this was not checked.
- **Table rows: 0 of 237.** `htmd` flattens every table in the corpus. On the synthetic set it kept 3 of 4 rows of a simple data table, so it does convert tables it takes for data. Whether the corpus holds a real data table that was flattened is **not known**; the three messages read by eye are newsletters and a forwarded article. The header-cell count added to the harness is the first check.
- **List items: 46 against 38** for `htmd`, and 322 for `html-to-markdown-rs`: the list-item count is a line-shape heuristic and counts lines the converter wrote that look like list items; neither figure was investigated.
- **The other three candidates stay rejected, now on corpus-wide numbers.** `htmd` faithful keeps raw HTML in place of Markdown (8,012 raw tags, 0 links, 0 headings, 73,542 extra words). `html2markdown` loses structure (282 of 361 links, 73 of 140 images, 1 of 32 headings, 4 of 38 list items) and has 514 extra words, which the first run's reading attributed to numeric entities left in the text. `html-to-markdown-rs` keeps 46 of 140 images and 1 of 32 headings, drops hidden preheader text, and has the most missing words (122, or 0.45%); its missing words rose from 111 to 122 when the HTML was preprocessed, which was not investigated.
- **A metric limit is visible in the synthetic set.** The case with Markdown-looking text (`[brackets](paren)`) is counted as a link by the Markdown counter (`links_md=3` against 2 in the HTML, for every candidate), and the destination remover therefore removes "paren": that is probably the 1 missing word every candidate shows there. It is inherent to the metric, not a converter fault. `html2markdown` and `html-to-markdown-rs` show a second missing word on the synthetic set that this does not explain.

### What this does not show

- The metric does not check order, so a converter that reorders text (for example while flattening a layout table) is not caught by it; only the by-eye review does, and only the three largest messages have been read.
- Corpus-wide counts do not say which words are missing, which messages have tables or hidden text, or what share of the output is link destinations. The per-input lists, the destination-byte count, the header-cell count, and the hidden-marker count were written for the next build, **not yet built or run**.
- Dependency weight and licenses (above); the corpus is one producer; no data-table, hidden-text, or tracking-URL policy is decided.

### Status

**Not closed.** The leading candidate is `htmd` 0.5.4 in default mode, behind the preprocessing described above; the choice is not recorded in the body-policy ADR, so the gate is not met. What remains is listed in the plan's "Decisions still needed" (item 1).

## M4e-0: bake-off re-run after the metric fix, and the per-input run (2026-10-11)

### Build, quality gate, and cost

At the metric-fix build the owner reported that everything built and **233 tests pass**. The per-input counts were then added, and the owner reports for the `v0.1.29` candidate: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (**236 passed**, 0 failed), and `cargo build --release` all succeeded; the CI result for the push is not yet reported. `cargo update` moved `syn` 3.0.8 to 3.0.9 and `unicode-ident` 1.0.26 to 1.0.27 (`Cargo.lock`); it reports `htmd` 0.5.5 and `html-to-markdown-rs` 3.17.2 as available, and the bake-off measured 0.5.4 and 3.11.0. `Cargo.toml` is at `0.1.28`; nothing is committed yet, and the owner plans to commit with updated documents and tag `v0.1.29`.

Release build times: **49.81 s** without the `bakeoff` feature and **54.95 s** with it (+5 s), against 47.60 s and 54.11 s in the first run. An earlier measurement the same day, 45.66 s and **1 min 25 s**, was an outlier (probably a rebuild after dependency updates; not checked), and the first draft's statement that the feature adds about 40 s is withdrawn. The cost of a clean build, and of `htmd` alone once it is a normal dependency, was not measured.

### Dependency weight and licenses

`cargo tree` below the first level (`htmd`'s subtree, taken from the full tree because `cargo tree -p htmd --features bakeoff` is refused for a package outside the workspace): 34 entries, 33 distinct crates (`syn` appears as 2.0.119 and 3.0.9): `htmd` 0.5.4 over `html5ever` 0.38, `markup5ever_rcdom` 0.38.0+unofficial, and `phf` 0.13, with, beneath them, `markup5ever`, `xml5ever`, `web_atoms`, `string_cache`, `tendril`, `parking_lot`, `lock_api`, `parking_lot_core`, `scopeguard`, `smallvec`, `log`, `siphasher`, `precomputed-hash`, `new_debug_unreachable`, `cfg-if`, `windows-link`, `fastrand`, `serde` (with `serde_core` and `serde_derive`), and the proc-macro stack (`proc-macro2`, `quote`, `syn`, `unicode-ident`). The `phf` generators (`phf_macros`, `phf_codegen`, `phf_generator`) and `string_cache_codegen` are build-time. Compared with the tree without the feature (which includes dev dependencies), **22 crates beyond `htmd` itself are new**; the other 11 entries are already in the tree. All three candidates together add 69 entries, among them second versions of `html5ever` (0.39) and `phf` (0.14) that exist only because of the feature.

`cargo license --features bakeoff`: `htmd` is Apache-2.0. Every crate in its subtree is MIT, Apache-2.0, or Apache-2.0 OR MIT. The tree has no copyleft licenses: the one LGPL mention (`r-efi`: Apache-2.0 OR LGPL-2.1-or-later OR MIT) is a choice that includes MIT and Apache-2.0, and `r-efi` is not in `htmd`'s subtree. Other licenses in the tree, all from the candidates the project is not keeping: Unicode-3.0 (the `icu_*` crates), CC0-1.0 (`tiny-keccak`), Apache-2.0 OR MIT OR Zlib (`tinyvec`). Not verified: the maintenance status of `markup5ever_rcdom`, whose version carries the suffix `+unofficial`.

### What changed in the harness

- **Word metric:** a multiset difference between the HTML's visible words and the Markdown's words (`words_missing`, and the new `words_extra`), after link and image destinations are removed from the Markdown. Order is not compared.
- **Preprocessing ("pre" variant):** now also removes `<style>` and `<script>` elements with their text, in one left-to-right pass with comments (a `<!--` inside a script is script text; a `<style>` inside a Word conditional comment is comment text); then Word's downlevel markers and `o:p` tags, as before.
- **Per-input counts:** the labels of the inputs that have missing or extra words, with how many (file numbers only); the bytes taken by link destinations; the HTML's header cells; and the HTML's tags hidden by `display:none` or `mso-hide:all`.

### The runs

`--bakeoff-md --bakeoff-dump C:\md1 C:\dump1 > bakeoff2.txt` (the first attempt failed with "failed to read the bake-off input directory" because `C:\dump1` did not exist at that path; it was re-run with the directory that holds the `NNN.custom.html` files), then `--bakeoff-md C:\dump1 > bakeoff3.txt` with the per-input counts. 27 recovered bodies and 16 synthetic cases, four candidates, raw and preprocessed variants. **0 panics, 0 errors, and 0 nondeterministic outputs for every candidate and variant over all 43 inputs.** All 361 keys that the two runs share are identical (timings excluded), so the additions changed no earlier figure. Every candidate shows `replacement_chars=1` on the corpus: that is the Windows-1255 byte from the de-encapsulation step, so it comes from the input, not from a converter.

Corpus totals, preprocessed variant (HTML side: 26,952 visible words, 361 links, 140 images, 32 headings, 38 list items, 237 table rows, 0 header cells, 16 hidden-style tags):

| Candidate | words missing | words extra | links | images | headings | list items | table rows | raw tags | noise | bytes | destination bytes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `htmd` | 2 | 262 | 361 | 140 | 31 | 46 | 0 | 1 | 0 | 264,071 | 86,187 (32.6%) |
| `htmd` faithful | 0 | 73,542 | 0 | 0 | 0 | 0 | 0 | 8,012 | 2,246 | 653,609 | 0 |
| `html2markdown` | 53 | 514 | 282 | 73 | 1 | 4 | 34 | 14 | 0 | 525,124 | 48,184 (9.2%) |
| `html-to-markdown-rs` | 122 | 1,440 | 345 | 46 | 1 | 322 | 44 | 15 | 0 | 254,996 | 66,387 (26.0%) |

Raw variant, for comparison: `htmd` 2 missing, 21,664 extra, 508 noise markers, 414,326 bytes; `htmd` faithful 0 missing, 126,083 extra, 11,748 raw tags, 3,761 noise markers; `html2markdown` 53 missing, 29,510 extra, 2,694 raw tags, 497 noise markers; `html-to-markdown-rs` 111 missing, 1,452 extra, 2 noise markers. Time over the 27 bodies (first run of each, milliseconds): `htmd` 32 raw and 20 preprocessed, `htmd` faithful 38 and 25, `html2markdown` 27 and 21, `html-to-markdown-rs` 76 and 35; not a differentiator.

Per input, `htmd` preprocessed: **missing words only in message 001 (2)**, the same 2 as in the raw variant; extra words in 13 of 27 messages: 001:19, 005:8, 007:26, 008:20, 009:9, 010:8, 014:18, 015:94, 020:8, 023:23, 024:21, 027:7, 029:1 (262 in all). The other candidates miss words in more messages: `html2markdown` in 001, 007, 009, 016, 023, 024; `html-to-markdown-rs` in 001, 007, 008, 009, 016, 023, 024, 027; `htmd` faithful in none.

Synthetic set, preprocessed: `htmd` 1 missing (case s16) and 3 extra (s04: 2, s07: 1); `htmd` faithful 1 and 10,684; `html2markdown` 2 missing (s12, s16) and 3 extra; `html-to-markdown-rs` 2 missing (s12, s16) and 3 extra. Every candidate reports `links_md=3` against 2 links in the HTML. Case s12 is the 300-deep nesting: `html2markdown` and `html-to-markdown-rs` lose one word there; `htmd` does not.

### What the numbers show

- **The `htmd` style-text defect is confirmed and removed by the preprocessing.** On the raw variant `htmd` has 21,664 extra words and 508 noise markers; after preprocessing, 262 and 0, and the output falls from 414,326 to 264,071 bytes. The structure counts (links, images, headings, list items) are the same on both variants. Style and script removal is therefore a requirement of the body pipeline (M4e-2), not an option.
- **The first run's figures are superseded.** The preprocessed `htmd` size recorded in the first run (388,068 bytes) still contained body style text that Word's comment removal had not caught; the figure is now 264,071. The first run's `words_missing` column stays withdrawn.
- **`htmd` default mode keeps almost every word and all the links and images.** 2 of 26,952 words are missing (0.007%), 361 of 361 links and 140 of 140 images are kept, 31 of 32 headings, no Office noise, 1 raw HTML tag. The 2 missing words are both in message 001 and are `month` and `¹`: the HTML has `$20/month</span><span class="footnote-anchor-email" ...>&#185;</span>` and `htmd` writes `month¹`. The harness splits words at every tag, including inline ones, while Markdown has no delimiter there, so one glued word counts as 2 missing words and 1 extra: **a metric artifact, not a lost word** (confirmed by reading both files). On this corpus the word metric therefore shows no word lost by `htmd`; message 001 is also where `html2markdown` and `html-to-markdown-rs` miss words (14 each), and `htmd` faithful, whose output keeps the HTML, misses none.
- **The extra words are mostly explained by the metric on the synthetic set**: ordered-list numerals (s04, 2 words) and image alt text (s07, 1 word). Whether the same holds for the corpus (015 with 94 extra words, 007 with 26) was **not checked**.
- **Table rows: 0 of 237, and 0 header cells.** `htmd` flattens every table in the corpus, and none of them has `<th>` cells. On the synthetic set it kept 3 of 4 rows of a table with header cells, so it does convert tables it takes for data. Whether the corpus holds a real data table without header cells that was flattened is **not known**.
- **Hidden text:** 16 tags in the corpus are hidden by `display:none` or `mso-hide:all`; which messages, and what the hidden text says, is **not known** (`htmd` keeps hidden text; `html-to-markdown-rs` drops hidden preheader text).
- **Link destinations are a third of `htmd`'s output:** 86,187 of 264,071 bytes (32.6%) over 361 links and 140 images, about 172 bytes each.
- **List items: 46 against 38** for `htmd`, and 322 for `html-to-markdown-rs`: the list-item count is a line-shape heuristic and was not investigated.
- **The other three candidates stay rejected, now on corpus-wide numbers.** `htmd` faithful keeps raw HTML in place of Markdown (8,012 raw tags, 0 links, 0 headings, 73,542 extra words). `html2markdown` loses structure (282 of 361 links, 73 of 140 images, 1 of 32 headings, 4 of 38 list items), has 514 extra words, and loses a word at 300-deep nesting. `html-to-markdown-rs` keeps 46 of 140 images and 1 of 32 headings, drops hidden preheader text, has the most missing words (122, or 0.45%, in 8 messages), and loses a word at 300-deep nesting; its missing words rose from 111 to 122 when the HTML was preprocessed, which was not investigated.
- **A metric limit is visible in the synthetic set.** The case with Markdown-looking text (`[brackets](paren)`, s16) is counted as a link by the Markdown counter (`links_md=3` against 2 in the HTML, for every candidate), and the destination remover therefore removes "paren": that is the one missing word every candidate shows there. It is inherent to the metric, not a converter fault. The inline-tag splitting above is the second known limit, and it probably also accounts for part of the corpus's extra words (not checked).

### What this does not show

- The metric does not check order; order was checked on four messages (the by-eye review below), not on the whole corpus.
- Which messages have data-like tables or hidden text, and whether the corpus extras are alt text, list numerals, or glued words.
- The corpus is one producer; nesting deeper than 300 and inputs larger than the corpus are untested; no data-table, hidden-text, or tracking-URL policy is decided; the maintenance status of `markup5ever_rcdom` and whether `htmd` 0.5.5 changes any figure.

### Further checks (2026-10-12)

- **CI:** both pushes of the `v0.1.29` work succeeded on `main` (about 1 min 29 s and 2 min 20 s); `v0.1.29` is tagged.
- **The 2 missing `htmd` words are a metric artifact.** Message 001's HTML has `$20/month</span><span class="footnote-anchor-email" ...>&#185;</span>`, and `htmd` writes `month¹`; the harness counts a word break at every tag, so it counts `month` and `¹` as missing and `month¹` as extra.
- **Re-score with inline tags glued** (a PowerShell check outside the harness: tags `span`, `font`, `sup`, `sub`, `small`, `u`, `mark`, `abbr`, `time`, `data`, `bdi`, `bdo`, `dfn`, `kbd`, `q`, `samp`, `var`, `big`, and `label` removed without a break; other tags become spaces; the same link-destination removal) over all 27 messages: **0 words missing in every message**; 243 extra words (the harness's 262 less 19 glue effects): 37 numerals (ordered-list numbering: 007 21, 015 8, 024 5, 008 3), 169 image alt text (015 86, 008 14, 024 14, 005 8, 010 8, 020 8, 001 7, 023 7, 027 7, 007 5, 009 5), and 37 first listed as unexplained (001 11, 023 16, 009 4, 008 3, 024 2, 029 1). The owner's listing for 001 and 023 and then a title check on six messages explained 36 of them: 28 are the document `<title>`, which `htmd` writes as the first line of the Markdown (001: 11 words; 023: 8; 009: 4; 008: 3; 024: 2 net of a repeated word), 8 are Cyrillic image alt text in 023 that is entity-encoded in the HTML (the listing script did not decode it), and 1 in 029 is not explained. 029 has no title; 007, 015, 022, and 027 have none or an empty one.
- **Hidden text:** a script matching elements with an inline `display:none` or `mso-hide:all` (outside comments, `head`, `style`, and `script`) found 15 elements in 7 messages with 82 words inside: 001 (2 elements, 4 words), 007 (2, 14), 008 (3, 8), 009 (2, 17), 023 (2, 24), 024 (2, 8), 027 (2, 7). The harness counts 16 tags; the difference was not chased. The text itself was not read for this record.
- **Template and table counts from the owner:** five messages (001, 009, 023, 024, 027) show 14 `display:none` occurrences each in a plain string count, which includes style sheets and comments, so they share a template. Tables: 024 (51 `<tr>`, 71 `<td>`), 001 (39, 73), 007 (33, 55), 023 (32, 52), 009 (30, 46), 008 (26, 33), 015 (16, 24), and 027 (2, 6); the other messages have at most 2 rows or none. No header cells anywhere.
- **`htmd` writes the `<title>` as the first line of the Markdown.** Confirmed on the six messages checked: in 001, 008, 009, 023, and 024 the first line of the Markdown equals the non-empty `<title>` text. This was not in the measured preprocessing, so the converter ADR is amended to remove `<title>` elements with their text in the same pass as comments, `style`, and `script`; the bake-off was re-run with it (see "M4e-2a: the pure body module" below): `htmd`'s corpus extra words fell from 262 to 233, nothing else changed.
- **Policies accepted by the owner:** see the plan's "Decisions in force". The converter ADR is drafted (`ADRs/html-to-markdown-converter-and-preprocessing.md`, proposed).

### By-eye review (2026-10-12): 007, 015, 022 (the smallest), 027

The owner supplied the four HTML and Markdown pairs. Method: the visible word sequence of each HTML file (comments, head, style, and script removed; inline tags with no Markdown delimiter glued) against each Markdown file (image alt text, link destinations, and list numerals removed), compared in order with `difflib`; plus counts of links, images, headings, lists, and tables, and a read of the hidden elements and of the Markdown.

| Pair | HTML to Markdown (bytes) | Words | In-order match | Links | Images |
|---|---|---|---|---|---|
| 022 | 40 to 17 | 3 | 3 of 3 | 0 of 0 | 0 of 0 |
| 015 | 40,737 to 3,911 | 223 | 223 of 223 | n/a | n/a |
| 007 | 65,159 to 31,769 | 2,181 | 2,181 of 2,181 | 49 of 49 | 20 of 20 |
| 027 | 43,497 to 9,087 | 253 | 253 of 253 | 7 of 7 | 6 of 6 |

**No difference of any kind in the word sequence in any of the four** (zero non-equal operations). No data table was flattened (007: 27 tables, 33 rows, all layout; 027: one table of 2 rows and 6 cells, layout; 015: a stack of embedded posts in layout tables), nothing was reordered, no link or image was lost, and no CSS, raw tag, or entity was left behind. Findings that change no decision:

- **022** is a one-line message (`<div dir="ltr">` around one sentence); the Markdown is that sentence.
- **015:** the avatar images' alt text equals their image URLs, which is the source of most of its 86 alt-text extra words. The corpus's only U+FFFD is here, present in the HTML before conversion: the known Windows-1255 limit of the de-encapsulation, not a converter defect. `cid:` images stay as `cid:` references (M4e-2 and M4f). A literal `<...>` in the text is escaped, and one link has empty text.
- **007:** 11 headings (h1, h3, h4), all present. One is nested in a list item and indented four spaces; the harness's heading count ignores lines indented that far, which is the corpus's "31 of 32". Ordered lists show doubled numerals (`1.  1.`) because the HTML carries a literal number in each item, and a "brought to you by" list renders as `*   \*` items inside a blockquote because the HTML carries a literal asterisk glyph; both are faithful to the source and look noisy.
- **007 and 027:** a hidden preheader sentence is kept as a plain paragraph, followed by a hidden padding element that survives as 200 combining grapheme joiners (U+034F), 200 soft hyphens (U+00AD), and about 200 no-break spaces, roughly 1.5 KB of invisible characters per message. This is the template shared by 001, 009, 023, 024, and 027, and it is a candidate for a normalization rule in the second ADR.

### Status

**M4e-0 complete (2026-10-12).** The choice (`htmd` 0.5.4, default mode, behind the preprocessing described above) is recorded in `ADRs/html-to-markdown-converter-and-preprocessing.md`, accepted by the owner, and the by-eye review found no defect. M4e (M4e-0 and M4e-1) is complete; M4e-2 (the body pipeline) is next and starts with the second body-policy ADR (the plan's "Decisions still needed", item 1).

## M4e-2a: the pure body module (2026-10-13)

`src/body.rs` (selection, native-HTML decoding, the preprocessing moved out of the bake-off with `<title>` removal and a record of the steps that applied, the invisible-padding collapse, `htmd` conversion on a large-stack worker thread, and the loss record) is built and tested on Windows. It is not called by the export yet (M4e-2b), so exported archives are unchanged. Changes beside the new file: `htmd` is a normal dependency pinned `=0.5.4` and no longer part of the `bakeoff` feature; `bakeoff.rs` calls `body::preprocess_html`; `main.rs` declares `mod body;`.

### Gate

The owner reports that `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, and both release builds succeeded. **272 tests passed, 0 failed** (the 236 before and 36 new, as expected). CI for the push is not yet reported.

| Build | Time |
|---|---|
| `dev` (clippy) | 35.33 s |
| `test` | 37.09 s |
| `release`, no `bakeoff` feature | 56.25 s (49.81 s before `htmd` joined the default build) |
| `release --features bakeoff` | 53.84 s |

The default release build is about 6 s slower, from a single measurement; the feature build runs after the plain one and reuses its compiled `htmd`, so the two figures are not independent.

### Bake-off re-run with the shared preprocessing

`--bakeoff-md C:\dump1 > bakeoff4.txt`, compared with `bakeoff3.txt` key by key (timings excluded). Only `_pre_` keys changed; the raw variants and every structure count (links, images, headings, list items, table rows, noise markers, replacement characters) are identical.

| Key | Before | After |
|---|---|---|
| `htmd` pre, words_extra | 262 | 233 |
| `htmd` pre, output bytes | 264,071 | 263,890 |
| `htmd` pre, words_missing | 2 | 2 (unchanged) |
| `htmd` faithful pre, words_extra | 73,542 | 73,499 |
| `htmd` faithful pre, raw HTML tags | 8,012 | 7,998 |
| `html-to-markdown-rs` pre, words_extra | 1,440 | 1,408 |
| `html-to-markdown-rs` pre, words_missing | 122 | 124 |

`htmd` extras by message: 001 19 to 8, 008 20 to 17, 009 9 to 5, 023 23 to 15, 024 21 to 18; the other eight messages are unchanged. That is 29 fewer, the title words of the five messages (11, 3, 4, 8, and 3), as predicted before the run (about 234). `html2markdown` did not change.

### What this shows

- **The title removal does what the ADRs said and nothing else.** `htmd`'s extras drop by exactly the title words, its missing words and structure counts do not move, and no other message changes.
- **The title words were not hiding a lost word for `htmd`.** Removing them left `words_missing` at 2, the `month¹` artifact.
- **The same removal exposed two words `html-to-markdown-rs` drops in message 023** (122 to 124 missing, 37 to 39 in 023). The likeliest reason is that the title's words had been covering two body words it loses (a multiset effect); this was not checked, and it does not affect the choice, since the candidate is rejected.
- **The remaining `htmd` extras** (233) are image alt text and list numerals, plus the 8 entity-encoded Cyrillic alt words in 023 and 1 unexplained word in 029.

### Not yet confirmed

The `body.rs` selection and loss paths are covered by unit tests with stand-in converters and by three real `htmd` conversions; they have not run on the corpus (M4e-2b and M4e-2c). The native-HTML decoding has no real data. The `html_conversion_failed` fallback has only been exercised through a stand-in that fails.

## M4e-2b - the body in the export (release v0.1.31): verified on the corpus (owner review pending)

Status as of 2026-10-14. The automated gate was run on the corpus and passed; the owner review of exported messages is pending, and the items under "Not done, or raised by the run" are open.

**Build finding (first build of the drop).** The build failed under `-D warnings` with `field selection is never read` for `BodyOutcome` in `src/archive.rs` (`-D dead-code` implied by `-D warnings`). Resolution: the field was removed. The source used is in `Rendered`, the kept files are decided before `BodyOutcome` is built, and nothing else needs the selection; a test that reads the field only to satisfy the lint was rejected because it would keep a field alive that has no user. `cargo update --verbose` on the same machine moved `equivalent` 1.0.2 to 1.0.3 and `teaspoon` 0.1.30 to 0.1.31; `htmd` 0.5.4 (0.5.5 available) and `html-to-markdown-rs` 3.11.0 (3.17.2 available) were unchanged because both are pinned with `=`. Advice recorded in `docs/plans/m4-plan.md` (Decisions still needed, item 9): update neither in this change; update `htmd` as its own change after the corpus run and `--verify-bodies` exist.

**What to run and report (the M4e-2b gate, in order):**

1. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`: clean; the number of tests passing (it was 272 at `v0.1.30`; expect more).
2. `verify-split.ps1` against the pre-change baseline: the default, `--verify`, and PST outputs byte-identical (no diagnostic code changed).
3. Export the corpus to a short `--out` and report the `export_*` lines. Expected: `export_readback_mismatches=0`; `export_body_loss_html_conversion_failed=0`; `export_body_loss_rtf_decompression_failed=0`; `export_body_source_native_html=0`; `export_body_source_rtf_html` equal to the count of exported messages with encapsulated HTML (27 of the 29 top-level files, plus any in the subdirectories); `export_body_files_kept_html` equal to that count; `export_body_files_kept_rtf` equal to the number of messages with an RTF; `export_body_files_kept_txt` equal to the number with a non-empty plain body. Record the new `export_tree_sha256` as the baseline (the `v0.1.25.1` value, `72d85534...c2e5`, no longer applies).
4. A second export into a different directory: the same `export_tree_sha256`; a repeat into the same directory writes nothing.
5. Reconciliation, counts only: every `metadata.json` has a `body` object; each kept file's size and SHA-256 equal its `originals` entry; the kept-file counts equal the report's.
6. Owner review of two or three exported messages (suggested: a newsletter-template message, to confirm the document title and the invisible padding are gone from `message.md` and present in `body.html`, and a smaller message).

**Results (run 2026-10-14 on Windows, corpus of 34 `.msg` messages: 29 top-level and 5 in subdirectories).**

1. **Build and tests: pass.** `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (293 passed, 0 failed; 272 at `v0.1.30`; includes `fixture_corpus_verify_is_clean`, which passed), `cargo build --release`, and `cargo build --release --features bakeoff` are clean. A second `cargo update --verbose` after the fix locked 0 packages.
2. **Diagnostic outputs unchanged: pass.** `verify-split.ps1`: the default output (850 bytes, 33 lines) and the `--verify` output (2705 bytes, 82 lines) are IDENTICAL to the baseline. `--verify` over the 29 top-level files: parity on every comparable field, `structural_gate_violations=0`, `rtf_decompressed_bytes_mismatch=1` with delta -1 (the known `msg_parser` dictionary divergence). The `verify-split.ps1` PST check printed `EMPTY (invalid check)` with 140 bytes on stderr; the cause has not been seen (no PST code changed in this release), so it is open.
3. **PST dry run at a short `--out` (an open item since v0.1.22): recorded.** At `C:\o.31` (root 18 units): 154 entries, 10 folders, 60 messages, 84 attachment files, longest path 253 of 259 units, `plan_gate_violations=0`, `plan_budget_exceeded=0`. At `C:\o.31.run2` (root 23 units) the totals are identical and the longest relative path falls from 235 to 230 units because names are shortened further. The PST counts are the same as at `v0.1.22`.
4. **Export of the `.msg` corpus: pass.** Counts reported by the first run (the second run into a different directory, and the repeat run into the first, printed the same body counts):

| Key | Value |
|---|---|
| `plan_entries_total` / `plan_messages` / `plan_folders` | 74 / 34 / 3 |
| `plan_gate_violations` / `plan_budget_exceeded` / `plan_max_relative_path_units` | 0 / 0 / 181 |
| `export_files_written` (first run) | 171 (4 `folder.json`, 68 `message.md` and `metadata.json`, 99 kept body files) |
| `export_readback_mismatches` | 0 |
| `export_bodies_plain_present` / `_empty` / `_absent` | 34 / 0 / 0 |
| `export_body_source_native_html` / `_rtf_html` / `_plain_text` / `_none` | 0 / 31 / 3 / 0 |
| `export_body_files_kept_html` / `_rtf` / `_txt` | 31 / 34 / 34 |
| `export_body_loss_tables_flattened` | 237 |
| `export_body_loss_rtf_not_converted` | 3 |
| `export_body_loss_undecodable_bytes_replaced` | 1 |
| `export_body_loss_invisible_padding_removed` | 1 |
| `export_body_loss_html_conversion_failed` / `_rtf_decompression_failed` | 0 / 0 |
| `export_body_cid_references_total` | 26 |
| `export_tree_sha256` | `dafb32909fa95df2be2c15b46add0bed03d1de9e2c8a784635d6ec52d34893c8` |

   The new baseline tree hash is the one above (the `v0.1.25.1` value `72d85534...c2e5` no longer applies). Reading the counts: 31 of the 34 messages carry encapsulated HTML (27 of the 29 top-level files, so 4 of the 5 in subdirectories) and use the HTML recovered from the RTF; the other 3 have an RTF with no encapsulated HTML, fall back to the plain text, and are recorded as `rtf_not_converted`, which is the same 3. No message has native HTML, none failed conversion, and none failed RTF decompression, as expected. The 1 undecodable byte is the known single Windows-1255 byte.
5. **Determinism and re-run: pass.** The export into `C:\o.31` and the one into `C:\o.31.b` (a different directory, root 12 and 14 units) have the same `export_tree_sha256`. Repeating the export into `C:\o.31` wrote nothing (`export_target_state=owned`, `export_preflight_identical=171`, `export_files_written=0`, `export_files_unchanged=171`, `export_consent=not_needed`).
6. **Reconciliation: pass (counts only).** 34 `metadata.json` files; every one has a `body` object (0 without); 31 `body.html`, 34 `body.rtf`, and 34 `body.txt` files, equal to the `export_body_files_kept_*` counts; of 99 kept files (31 + 34 + 34), the size and SHA-256 of every file equal its `originals` entry (0 mismatches, 0 missing).

**Not done, or raised by the run (carried forward):**

- **Owner review of exported messages (gate item 6) has not been reported.** It is the check that the document title and the invisible padding are gone from `message.md` and present in `body.html`, and that the Markdown reads well.
- **Padding: one run was removed in the whole corpus.** The body ADR evidence expected padding in at least two messages of the same newsletter template. Possible reasons are that `htmd` already drops or rewrites some of the characters, or that only one exported message carries a run of 8 or more; neither has been checked. A counts-only check (padding characters remaining in `message.md`, and in `body.html`) is in the release notes of this change; the originals are kept, so nothing is lost either way.
- **`tables_flattened` is 237 rows over the 31 HTML messages.** It is recorded loss, not a failure, and is consistent with layout-table newsletters. The ADR question remains open: whether the count should be of rows or of tables, and how large a count should become a status concern at M4g.
- **The `verify-split.ps1` PST check is EMPTY** (cause unseen).
- **Not covered by this gate:** the content-preservation check over the corpus (`--verify-bodies`, M4e-2c).

**Premises this change could not check** (the author had no Rust toolchain): that `compressed-rtf` accepts an MS-OXRTFCP uncompressed (`MELA`) container (two export tests begin with a check that fails with a clear message if it does not); the exact Markdown `htmd` produces for the small HTML in the export tests (assertions use `contains`); and `rustfmt` output (run `cargo fmt`).

**Open items carried forward:** `--verify-bodies` (M4e-2c); the native-HTML path on real data (the corpus has none); a string-typed `PidTagBodyHtml` is not read; `rtf_decompression_failed` was 0 on the corpus; the streaming export (M4h-0) for multi-gigabyte inputs; the `htmd` update after the evidence exists; the items listed under "Not done, or raised by the run".

## M4e-2c: `--verify-bodies` (v0.1.32, 2026-10-15)

Built clean, 312 tests pass (293 plus 19 new in `src/verify_bodies.rs`). `cargo fmt --check` and `clippy -D warnings` output and the corpus export tree hash were not reported for this tag.

Command: `tsp --verify-bodies --recursive <corpus>` (34 files, 0 subdirectories skipped):

```text
open_errors=0
messages_read=34
body_selected_native_html=0
body_selected_rtf_html=31
body_selected_plain_text=3
body_selected_none=0
html_bodies_checked=31
html_conversions_failed=0
words_html_total=26145
words_markdown_total=26385
words_matched_in_order=26145
words_missing_from_markdown=0
words_extra_in_markdown=0
markdown_syntax_words_excused=240
missing_runs=0
extra_runs=0
unresynced_words=0
html_unresolved_entities_total=0
messages_identical=31
messages_with_difference=0
body_check_gate_violations=0
```

Reading: every HTML word of the 31 checked bodies appears in the Markdown in the same order, and every Markdown word is either matched or one of 240 syntax words (26,145 + 240 = 26,385). The totals are not comparable with the M4e-0 bake-off's 26,952 words over 27 bodies (different tokenization: the bake-off split at every tag and counted title, head, and padding text; this check joins inline tags and removes them), and the two were not reconciled.

Limits: the check starts from the HTML after the production preprocessing, so text removed by the preprocessing is not covered; emphasis and padding characters are stripped on both sides by design; the 240 excused words were not listed; one producer; the comparison is greedy (zero is exact, a non-zero count would be approximate). Differences from the wording of ADR decision 6 are listed in the plan (v4.21, "What changed from v4.20", item 5).

`verify-split.ps1 -Pst tsp-tester.pst` (baseline v0.1.19.1 against the new build): default 850 bytes / 33 lines, `--verify` 2705 / 82, PST diagnostic 1092 / 44, all IDENTICAL with empty stderr; `structural_gate_violations=0`, `rtf_decompressed_bytes_mismatch=1`, `embedded_message_class_readable_total=1`. The EMPTY PST check recorded at v0.1.31 did not recur.

## M4h-0: the streamed export (v0.1.33, 2026-10-17)

Quality gate: `cargo fmt --check` and `clippy -D warnings` clean; 321 tests pass, 1 ignored (the generator of the large corpus); release builds with and without the `bakeoff` feature clean. `Cargo.toml` stays at 0.1.32. `--verify-bodies` unchanged (26,145 matched, 0 missing, 0 extra); `verify-split.ps1`: default, `--verify`, and PST outputs IDENTICAL.

Corpus export (34 messages, short `--out`): 171 files written, `export_readback_mismatches=0`, `export_tree_sha256=1daa76fc2bb7ff8e4077e8b54888e44362ef137c815d241ca11e81faf277d53f`; the same hash for an export into a second directory and for a repeat run (171 identical, 0 written); no `.tsp-tmp` left. Worktrees of exactly `v0.1.31` and `v0.1.32` (fresh `git worktree add`; `v0.1.32` is commit `083630e`) print the same hash. An earlier attempt used a folder named `teaspoon_v0.1.32` that was not a git worktree (no `.git`, no `stage.rs`); it printed the same hash too, but is not counted as evidence because its commit is unknown.

The `dafb32909fa95df2be2c15b46add0bed03d1de9e2c8a784635d6ec52d34893c8` recorded above for `v0.1.31` (2026-10-14) was not reproduced: both old tags now print `1daa76fc...`. `Cargo.toml` says 0.1.32 at both tags. The cause is not identified (candidates: the tool version recorded in the root `folder.json`, the input folder name, or the fixture files); `1daa76fc...` is the baseline from here.

Consent paths by hand on real output (an edited `message.md` and two files of the owner's own added to the archive; counts and exit codes only):

| Case | exit code | consent | result | files written | `.tsp-tmp` after | message.md original | added files |
|---|---|---|---|---|---|---|---|
| non-interactive, no `--overwrite` | 2 | `non_interactive_declined` | `refused_needs_consent` | 0 | absent | no (edit kept) | kept |
| interactive, `n` | 2 | `prompted_declined` | `refused_declined` | 0 | absent | no (edit kept) | kept |
| interactive, `y` | 0 | `prompted_accepted` | `completed` | 2 | absent | yes | kept |
| `--overwrite` | 0 | `overwrite_flag` | `completed` | 2 | absent | yes | kept |
| directory `tsp` did not create, `--overwrite` | 2 | not applicable | `refused_target_not_owned` | 0 | absent | not applicable | its file untouched |

In the first four cases the preflight reported 1 file to replace and 2 unrelated entries.

Memory (synthetic corpus: 300 messages, each with a 25 MB HTML body and a plain-text body, about 45 MB per message, 15.7 GB, generated in 107.64 s; `measure-peak.ps1`, which polls the process):

| Build | Messages | Peak working set | Peak paged memory | Elapsed | Files written | Read-back mismatches | `export_tree_sha256` |
|---|---|---|---|---|---|---|---|
| old (`v0.1.32`) | 20 | 1,668 MB | 2,089 MB | 41.1 s | 81 | 0 | `12477df68c45ee8994257db9bf219d477eda6d5c36f5fa0acd35710d6c12ca36` |
| new (streamed) | 20 | 521 MB | 535 MB | 37.9 s | 81 | 0 | `12477df68c45ee8994257db9bf219d477eda6d5c36f5fa0acd35710d6c12ca36` |
| new | 50 | 522 MB | 541 MB | 92.2 s | 201 | 0 | `c84acd8f71d651ac2ae86e13d6cf288d90edabebe555b2cca76baa8a38252103` |
| new | 300 | 523 MB | 546 MB | 888.5 s | 1,201 | 0 | `26f7218e0991dcdc1530ff59d507f24d2ba901ea410d6b95afc47a794beb034e` |

Reading: the streamed writer's peak is flat (15 times more messages, about 0.4% more memory) and the old and new writers produce the same hash at 20 messages. The old writer's growth works out to about 58 MB per message; the old build was not run above 20 messages, so any figure for 300 would be an extrapolation (on the order of 18 GB). The 520 MB that remains is the cost of converting one 25 MB message.

Limits: time per message rose from about 1.9 s (20 and 50 messages) to about 3.0 s (300) with flat memory (cause not measured; disk and antivirus load are the likely reason); only 1,201 files were staged in the flat staging directory and no manifest at millions of files was tried; the messages are synthetic and uniform; a whole message is still held while it is rendered.
