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

## M4c: walking skeleton (built 2026-10-03; tag `v0.1.23.1`, `Cargo.toml` version `0.1.23`)

### Build and regression

- Built clean; **155 tests pass** (117 existing plus 38 new); committed and tagged `v0.1.23.1`. Added dependencies: `serde`, `serde_json`, `sha2`.
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

### Not yet run

- The non-interactive refusal on real output (edit a file, re-run with input redirected, e.g. `$null | tsp.exe ...`; expect `export_result=refused_needs_consent` and exit code 2), and the `--overwrite` path replacing a file in that situation. Both are covered by synthetic tests.
- Whether CI (Linux and Windows) passes on the pushed commit.
- `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` on the final commit (not reported).
- A PST dry run with a short `--out`, and the current PST diagnostic output (both still open from M4b-3).

### Status

M4c is built, passes its synthetic tests, has exported the real `.msg` corpus with counts that reconcile and a stable tree hash, and one exported message has been reviewed by the owner. The quality-gate and CI reports above are outstanding. The M4a-2 decision records and the full M4b-4 model are not done, so the `0.1-draft` schemas are not frozen.
