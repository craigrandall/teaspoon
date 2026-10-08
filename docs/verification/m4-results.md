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

At `v0.1.26` the loop over a folder's sub-folders in `PstBuilder::folder` appears **twice** in `source_pst.rs` (two identical `if depth < MAX_FOLDER_DEPTH && let Some(hierarchy) = folder.hierarchy_table() { ... }` blocks, apparently from the edition migration). Each sub-folder is opened and pushed twice, and each of those copies repeats the doubling for its own sub-folders, so a folder at depth `d` is planned `2^d` times; the folder identity counts double as well. The 201 tests do not cover it (no PST fixture is in the repository), and no PST dry run was reported at `v0.1.26`; the one reported on 2026-10-07 was at `v0.1.25.x`, before the migration. The default PST diagnostic (`pst.rs`) and `--verify` are unaffected. Fix: delete the second block. Check: the PST dry run with a short `--out` should again report `plan_folders=10`, `plan_messages=60`, `plan_attachment_files=84`, `plan_entries_total=154`, and `folder_identity_nid_available=11`.

### Gate for M4e-1

| Gate item | State |
|---|---|
| Spec-derived unit tests, and the property tests (arbitrary input never panics, output bounded) | **met**: they are part of the 201 passing tests |
| Weak-oracle differential against `msg_parser`'s `html_from_rtf()` on the 27 encapsulated-HTML messages, counts only, every disagreement triaged | **not met**: `--verify-deencap` output not yet reported |
| Recognition rule against `check_compressed_rtf_bytes` | **not met**: compared by `--verify-deencap`; not yet reported |
| Hand comparison of a sample against a reference implementation | not done |

What to expect from the first `--verify-deencap` run, stated as hypotheses to check: one recognition disagreement (the genuinely RTF-authored fixture, which `msg_parser` and the whole-document marker search treat as HTML but the 10-token rule does not), some disagreements from `msg_parser`'s Latin-1 treatment of `\'hh` escapes, one file affected by the dictionary divergence, and unsupported-code-page bytes (`deencap_unsupported_codepage_<n>_files`, `deencap_undecodable_bytes_total`) if any message uses a code page other than 1252.

### Identifiers and recipient types (2026-10-07)

Every property identifier in the M4d envelope was checked against Microsoft's pages (the MAPI canonical property pages, and MS-OXPROPS for 0x0C1A) and is tier A. Sensitivity values 0 to 3 and importance high = 2 were seen in Microsoft text; importance low = 0 and normal = 1 were not. **A likely defect:** Microsoft's PidTagRecipientType page says the value is one type plus an optional flag (MAPI_P1, MAPI_SUBMITTED), but the envelope lists only values 1, 2, 3, so a flagged recipient would be dropped from `message.md` (counted in `recipients_unlisted`). The corpus has none (`recipients_unlisted_total=0`), so the differential run could not catch it. Details: [`../plans/m4d-envelope-properties.md`](../plans/m4d-envelope-properties.md).
