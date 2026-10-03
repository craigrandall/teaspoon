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
- **PST over-budget entries are the expected effect of the long output path.** At a root of 152 units, four entries cannot fit in 259 even at the planner's name floors, and the gate reports exactly those four (`plan_budget_exceeded=4`, `plan_gate_over_budget=4`, `plan_gate_violations=4`). The `.msg` run, with a root of 146 and a longest relative path of 107, fits (253 ≤ 259). The planner therefore reports a problem rather than hiding it; what an export does about it is an M4c decision. **Not yet run:** a dry run with a short `--out`, which should show 0 violations on both inputs.
- **Nameless attachments match the embedded messages the PST side cannot open.** `source_attachments_without_name`, `source_embedded_attachments_not_opened`, and `plan_names_fallback` are all 3 on the PST, and the planner names those entries by fallback.
- **Collision handling works on real names.** The PST has 3 collision groups (largest 16) and `plan_gate_collisions=0`.
- **Folder identity is available on the PST.** Both a node ID and an entry ID were readable for all 11 folders walked, none unavailable. The planned folder count is 10 and 11 were examined, which is consistent with one folder (the root) being identified but not exported; this has not been confirmed separately.
- **Duplicate Internet message IDs are common in the fixtures** (4 of 34 messages in the `.msg` directory, 20 of 60 in the PST), so an Internet message ID cannot be used as a unique key.

### Open: counts that differ from what was predicted

The M4b-3 plan predicted, from earlier diagnostics, 9 planned folders, 57 messages, and 79 attachments for the PST, and 29 messages, 1 embedded message, and 29 attachments for the `.msg` directory. The census reports 10 / 60 / 84 and 34 / 3 / 34.

- For the `.msg` directory the likely explanation is scope: the earlier counts covered the 29 top-level files, while the dry run plans recursively and mirrors 3 subdirectories. The split of the extra 5 messages and 2 embedded messages across those subdirectories has not been checked.
- For the PST there is **no confirmed explanation**: +1 folder, +3 messages, +5 attachment files. Either the fixture changed after the earlier diagnostic, or the planner walks something the diagnostic does not. The current PST diagnostic output (`tsp tsp-tester.pst`) is needed to settle it, and the PST export stage (M4i) is gated on the archive reconciling with the diagnostic.

### Status

M4b-1, M4b-2, and M4b-3 are done and verified, with the open items above. The M4a-2 decision records and the M4b-4 model types, which the plan places before M4c, are not done.
