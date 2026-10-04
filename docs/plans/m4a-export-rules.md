# M4a — export naming, layout, and overwrite rules (v3.5)

Status: **the rules in sections 4-6 (naming, path budget, uniqueness; N/L/U) are decided by the accepted ADR [Export naming, collisions, and path budgets](../../ADRs/export-naming-collisions-and-path-budgets.md); sections 2 and 8 (consent, ownership, lifecycle) by the accepted ADR [Export output posture](../../ADRs/export-output-posture-consent-and-ownership.md); section 9 (identity) by the accepted ADR [Message and folder identity and provenance](../../ADRs/message-and-folder-identity-and-provenance.md); and the rules every archive file follows (section 3.2) by the accepted ADR [Archive file contract and schema evolution](../../ADRs/archive-file-contract-and-schema-evolution.md). All four were accepted by the owner on 2026-10-04. This document remains the detailed text behind them.** The naming, path-budget, and uniqueness rules are implemented as pure code (`naming.rs`, `plan.rs`; M4b-2, verified by unit and property tests) and exercised by the `--dry-run` census (M4b-3, v0.1.22). The consent, staging, and write rules and a first version of the layout and metadata files are implemented for `.msg` input in M4c (v0.1.23, tag v0.1.23.1; verified on the real `.msg` corpus). Results: [`../verification/m4-results.md`](../verification/m4-results.md). The full metadata schemas, attachments, PST export, `--long-paths`, and `--max-relative-path` are not implemented, and the body and formatting-loss policy (section 11, item 4) is not yet decided. It completes what the accepted ADR [Deterministic Markdown archive](../../ADRs/deterministic-markdown-archive.md) explicitly left open (message identity, filename sanitization, collision handling, attachment relationships), and **does not change that ADR's layout**: one folder per message containing `message.md`, `metadata.json`, and an `attachments/` subfolder. Every "verify" tag marks a claim taken from documentation or memory that has not been checked against this project's fixtures or Windows.

Revision history: v1 (2026-09-30) proposed a different layout (`<subject>.md` plus `<subject> - attachments/`). The project owner decided to keep the accepted ADR's layout, so v2 adapted every rule to it and withdrew the ` - attachments` folder requirement (`attachments/` is a literal name). v3.5 (2026-10-04) records that the four ADRs this document implied were accepted; no rule changed. v3.4 (2026-10-03) recorded what M4c implemented and where it chose differently from the draft text (marked "as built" below). v3.3 (2026-10-03) recorded the status of implementation and the census results; no rule changed. v3.2 (2026-10-02) adds the subject-prefix marker to N1 and records what the crate source shows about folder identity and attachment names (M4b-3). v3.1 (2026-10-01) aligned the rules with the first implementation (M4b-2): U+2028/2029 become spaces, folders sort before messages in a shared namespace, and rule N6 is implemented. v3 (2026-09-30) records the owner's decisions on Q7-Q11: one uniform zero-padded ` (nn)` duplicate suffix (no ` - Copy`), export roots for non-PST inputs including mirrored subdirectories, SHA-256 of the source recorded by default, and content identity moved out of names into metadata.

Sources: the owner's requirements and decisions (2026-09-30); the external research "Support for linting a PST, anticipating a Windows filesystem for message export" (2026-09-29; *the research*); Microsoft's Win32 naming documentation and MS-PST; the existing ADRs.

## 1. Requirements and decisions in force

1. Exporting a PST mirrors its folder tree as a directory tree.
2. Each message becomes a folder (per the accepted ADR) whose **name mirrors the message subject**; the folder holds `message.md`, `metadata.json`, and `attachments/`.
3. Duplicate names are disambiguated with one uniform, zero-padded suffix, ` (02)`, ` (03)`, ... by position in a deterministic order; the first item keeps the bare name (decision A11). Content identity is recorded in metadata, not in names.
4. All PST-versus-Windows differences, including path length, are handled up front.
5. Sibling PST folders with the same name are told apart using the folders' own identifiers.
6. If an export could overwrite existing content, `tsp` warns and asks permission to continue, or exits.
7. Stdout stays content-free (counts only); archive content goes only into the directory the user names.
8. Export root is `<out>/<PST file stem>/` (sanitized), so several PSTs can share one `<out>` (Q1).
9. Metadata lives alongside the item it describes: `metadata.json` in a message folder (per the ADR), `folder.json` in a folder directory (Q3).
10. Replacement characters as proposed in N3 (Q4); scope as in section 12 (Q5); inline attachments are kept in `attachments/` and marked `inline` (Q6).
11. The default path budget is 259 units from the real root; `--long-paths` is an opt-in and `--max-relative-path N` a cap for archives that will move; the longest relative path is recorded and reported (Q7). Folder markers are `folder.json` (Q8). A SHA-256 of the source is recorded by default, `--no-source-hash` skips it (Q10).

## 2. What "CLI shape" means, and the proposal

"CLI shape" is how the user asks for an export on the command line, not what is written to disk. `tsp <input>` prints a content-free diagnostic and `tsp --verify <input>` compares two parsers. Export writes message content, so it needs its own spelling without disturbing existing usage or the stable output vocabulary.

```text
tsp <input>                          # unchanged: content-free diagnostic
tsp <input> --out <dir>              # export                                              (implemented for .msg input, M4c)
tsp <input> --out <dir> --dry-run    # project the export, report counts, write nothing   (implemented, v0.1.22)
tsp <input> --out <dir> --overwrite  # non-interactive consent to replace files this tool would generate   (implemented, M4c)
tsp <input> --out <dir> --no-source-hash   # skip the SHA-256 of the source file           (implemented, M4c)
tsp <input> --out <dir> --long-paths       # opt in to paths beyond 259 units              (not implemented)
tsp <input> --out <dir> --max-relative-path N   # cap relative paths for archives that will move   (not implemented)
```

Exit codes (as built): 0 completed, 2 refused with nothing written, 1 an error.

Consent behavior (requirement 6, as built in M4c):

- Preflight compares the plan with what is already under `<dir>/<stem>/` and reports **counts only**: files that would be created, files that already match, files that would be replaced, planned paths that something else occupies (`blocked`), and entries present that this export would not touch.
- No consent is needed when nothing would be replaced, including a repeat of an unchanged export (which writes nothing).
- Interactive terminal and files to replace: print the counts on standard error and ask `Continue? [y/N]`; anything but an explicit yes exits with code 2 and writes nothing.
- Not interactive (stdin is not a terminal): treated as "no" unless `--overwrite` was given.
- `tsp` only replaces files that its own plan generates. It never deletes entries it did not create. Deleting stale generated entries would be a separate `--clean` with its own consent; propose deferring it.
- Ownership is recognized by the root `folder.json` (section 3.2). **As built (and accepted in the output-posture ADR):** a target directory that exists, is not empty, and has no root `folder.json` written by `tsp` is refused, even with `--overwrite`; a root `folder.json` for a different source (different kind or name) is refused, even with `--overwrite` (the original draft called this "a stronger warning"). Source identity is kind plus name; the source hash is not compared, so re-exporting a changed source is allowed with consent.
- A plan that breaks a gate (collisions, an over-budget path, invalid names) is refused before any preflight, with nothing written.
- Writes go to a staging area and are moved into place (section 8).

## 3. Layout

```text
<out>/
  <PST file stem>/
    folder.json                      # root: source provenance, archive summary
    Inbox/
      folder.json
      Budget review/
        message.md
        metadata.json
        attachments/                 # present only when at least one file is written
          Q3 plan.xlsx
          image001.png               # inline: true in metadata.json
      Budget review (02)/            # second item with this name, by (time, identifier) order
      RE_ Budget review/             # ':' replaced
      RE_ Budget review (02)/        # second item with that name
      Projects/
        folder.json
    Sent Items/
      folder.json
```

(As built in M4c, for `.msg` input: everything above except `attachments/`, which is never created yet. A worked example is in [`../examples/m4c-example-archive.md`](../examples/m4c-example-archive.md).)

### 3.1 What maps to what

| PST | Filesystem |
|---|---|
| Store / input | `<out>/<PST file stem>/` with root `folder.json` |
| Folder | Directory named from `PidTagDisplayName`, containing `folder.json` |
| Message | Directory named from `PidTagSubject`, containing `message.md`, `metadata.json`, and (when needed) `attachments/` |
| Attachment (file) | File inside the message's `attachments/` |
| Embedded message | A message directory (with its own `message.md`, `metadata.json`, `attachments/`) inside the parent's `attachments/`; depth-capped (section 7) |
| Identity and provenance | `metadata.json` (messages) and `folder.json` (folders) |

Inputs other than a PST (decision Q9): a directory of `.msg` files exports to `<out>/<directory name>/`, **mirroring its subdirectories as folders** (each with a `folder.json`) with one message directory per `.msg` file; a single `.msg` exports to `<out>/<msg file stem>/` containing one message directory. The name of the source file goes into provenance, never into the message directory name. Mirroring subdirectories applies to export only: the diagnostic's non-recursive scan (which reports `subdirectories_skipped`) is unchanged, and the export plan reports how many subdirectories it mirrored (`source_subdirectories_mirrored`; implemented). A trailing path separator on the input does not change the name (`C:\mail\msgs\` and `C:\mail\msgs` both give `msgs`).

### 3.2 Metadata files

- **`metadata.json`** (message): as in the accepted ADR, it carries whatever the normalized model captured, including properties Markdown cannot render, extraction diagnostics, recipients, attachment records (including `inline: true/false`, content hash, status), provenance, the message's identity, its content SHA-256, and `identical_to` (identities of earlier messages with the same content hash). **As built (draft `0.1-draft`):** `schema_version`, `kind`, `source_file` (the `.msg` file's name or its `/`-separated path relative to the input directory, never absolute), `subject`, `internet_message_id`, `time_filetime`, `directory` (the original name and the adjustments the naming rules made, as snake_case labels), `body` (plain text `present`/`empty`/`absent`, how it is written, and whether native HTML, HTML-in-RTF, and RTF forms exist), `attachments_not_extracted`, `status` (always `partial` in M4c), and `status_reasons`. The remaining fields above arrive with M4d-M4g.
- **`folder.json`** (folder): original display name and original path, the folder's identifier (section 9), applied renames and truncations (with reasons), counts, and an index of child entries (kind, directory name, identity, and for messages the content SHA-256). The index supports overwrite preflight and read-back verification without scanning file contents. **As built:** `schema_version`, `kind`, `directory` (original name and adjustments), and `children` (kind and directory name only). Identity, counts, and hashes arrive with M4g.
- **Root `folder.json`**: additionally the source (file name, size, and its SHA-256 unless `--no-source-hash` was given), tool version, schema version, the path budget used (and `--long-paths` or `--max-relative-path` if set), the longest relative path, and archive-wide summary counts. **As built:** `kind` is `archive_root`, and a `root` object holds `tool` (`teaspoon`), `tool_version`, `status` (`incomplete` while an export runs and after an interrupted one, `complete` after), `source` (`kind` `msg_file` or `msg_directory`, `name`, `size_bytes`, `sha256`, and its scope: the file, or for a directory the hash of the sorted `<relative path>\t<file hash>` lines), `max_path_units_allowed`, `planned_longest_relative_path_units`, and `counts`.
- Schemas are versioned, ordered, and free of absolute paths and export-time values so two exports of the same input are byte-identical. These file rules, and the policy for changing the formats, are fixed by the accepted file-contract ADR. M4c's field-level schemas are marked `0.1-draft` and are frozen only when the metadata stage (M4g) assigns the first numbered version.

**Reserved names.** `folder.json` is reserved in every folder directory, and the staging name `.tsp-tmp` is reserved directly under `<out>`. A child directory whose collision key equals a reserved key is renamed by the uniqueness rules (N9). Message directories contain only the three fixed names, so no subject-derived name can collide there. (In the code today, `folder.json` is reserved by the planner. `.tsp-tmp` is not reserved by the planner: it lives beside the archive root, not inside it, and the only way it could clash is an input whose sanitized name is exactly `.tsp-tmp`, which is untested.)

**Why metadata alongside, and not in a hidden store or one root manifest** (a question the owner delegated): (1) the ADR's stated benefit is that each message "can be inspected, diffed, or moved as a unit", and a separate store or a single manifest breaks that; (2) a hidden directory is dropped by some copy tools and is one more tool-owned name to protect; (3) a single root manifest is a single point of failure and grows with the archive. The cost is one small file per folder, which is negligible next to the two or three files per message.

## 4. Deriving and sanitizing names

Applied to PST folder names, message directory names, attachment file names, and (for the root) the PST file stem. All rules are pure functions.

**N1. Source strings.** Folder: `PidTagDisplayName`. Message: `PidTagSubject`, the full subject, which per MS-OXCMSG is the subject prefix plus the normalized subject (so `RE: ` is kept). Attachment: `PidTagAttachLongFilename`, then `PidTagAttachFilename`, then a synthetic name (N8). For a PST the planner can only read the attachment table, which may carry just the short (8.3) name; the census reports how often a long-name column exists. Embedded message directory: the embedded message's subject. `PidTagSubject` can begin with U+0001 plus one more character that records the length of the subject prefix (the `outlook-pst` examples strip exactly these two characters); they are removed before sanitizing, and the census counts how often (`source_subject_markers_stripped`). Without this step N3 would delete the first character but the second could survive as a visible one.

**N2. Unicode form.** Normalize to NFC. Unpaired surrogates in PST UTF-16 data cannot be represented in a Rust string; decode with replacement and flag the item (`name_lossy_decode`). ANSI PST strings use the code page chain already built for `PT_STRING8`; an unsupported code page yields a synthetic name (N8) and a flag.

**N3. Characters removed or replaced** (replacement characters accepted by the owner).

| Class | Action |
|---|---|
| `< > : " \| ? *` | replace with `_` |
| `/` and `\` | replace with `-` |
| C0 controls U+0000-U+001F, DEL U+007F, C1 controls U+0080-U+009F | delete, flag |
| Bidi controls U+202A-202E and U+2066-2069, zero-width characters U+200B-200D and U+FEFF | delete, flag |
| Whitespace of any kind, including tabs, newlines, U+0085, U+00A0, U+2028/2029, and U+3000 | becomes a space, then runs collapse; leading and trailing spaces are trimmed |
| Trailing `.` or space (after all other steps) | trim repeatedly |

The replacement table is fixed permanently: changing it would rename every exported directory.

**N4. Reserved device names.** Compare the part of the component before the first `.` (after trimming), case-insensitively, against `CON PRN AUX NUL COM0-COM9 LPT0-LPT9`, the superscript variants, `CONIN$`, and `CONOUT$`. On a match, append `_` to that part (`NUL` becomes `NUL_`). This applies to **directories too**: a message whose subject is `NUL` is exported as a directory, and Windows still treats `NUL` as a device name. Verify the set against current Microsoft documentation; the conservative superset costs nothing.

**N5. Dot names.** A component that becomes `.` or `..` is treated as empty (N8).

**N6. Explorer-special and tool-special names.** Attachment names equal (case-insensitively) to `desktop.ini`, `thumbs.db`, `autorun.inf`, or beginning with `~$` get a leading `_` (flag `SpecialNameProtected`). Verify the list. Executable attachment types written to disk may be quarantined by antivirus; document it.

**N7. Idempotence.** `sanitize(sanitize(x)) == sanitize(x)`. Property-tested.

**N8. Empty or unusable names.** Empty after sanitizing: folder `(unnamed folder)`, message `(no subject)` (what Outlook displays), PST stem `archive`, attachment `attachment-<n>` (1-based position in the attachment table, with an extension from the MIME type only if one is recorded). Never omit an item.

**N9. Reserved for the tool.** Names that collide with reserved keys (`folder.json` in any folder directory; `.tsp-tmp` under `<out>`) are renamed by U4 as if they collided with a sibling.

**N10. Extensions (attachments only).** The extension is the text after the last `.` if it is 1-16 characters with no spaces; otherwise none. Suffixes go before the extension (`image001 (02).png`). Message and folder directories have no extension, so suffixes go at the end; this removes a class of extension-splitting bugs that a `<subject>.md` layout would have had.

## 5. Length and path budgets

**Measurement.** Windows limits count UTF-16 code units, not characters or bytes; an emoji counts as 2. Every length in these rules is UTF-16 code units. Verify with astral-plane tests.

**L1. Component limit.** 255 units per component on NTFS, FAT32, and exFAT, after all suffixes.

**L2. Path limit.** Default budget: the full path of every file and directory, including drive, separators, and the terminating null, stays under 260, so at most 259 units. Two wrinkles (verify against Microsoft documentation): (a) `CreateDirectory` reportedly enforces 248 for directory paths so an 8.3 name can still fit; treat directory paths as at most 247; (b) the user's `--out` path and the PST-stem directory count, so the relative budget is computed from the real absolute root.

Because this layout makes **every message a directory**, the 248 rule is the binding constraint for message directories.

**L3. Worst-case child reservation.** Name shortening for a message directory is computed from the worst-case path inside it:

- directory path `D` (absolute);
- `metadata.json`: `D + 1 + 13`; `message.md`: `D + 1 + 10`;
- attachment: `D + 1 + 11 + 1 + A` where `A` is the longest sanitized attachment name in that message including any duplicate suffix;
- embedded message: the same applies recursively from its own directory;
- plus room for the duplicate suffix on `D`'s last component: 6 units (` (999)`). The plan knows every group's real size, so it reserves the actual suffix width; groups of 1,000 or more are widened and re-truncated.

So `D <= 245` for `metadata.json`, and `D + 13 + A <= 259` when the message has attachments. Attachment names are first limited to 100 units including extension (stem floor 16), then message directory names are shortened. The stem for the PST root directory and staging names also count.

**L4. Truncation order.** When the projected path exceeds the budget: (1) shorten attachment stems to at most 100, then as needed to a floor of 16; (2) shorten message directory names, longest first, to a floor of 24; (3) shorten folder names, deepest first, to a floor of 16; (4) flatten: join an over-budget chain of folders into one directory name `Parent - Child - Grandchild` and record the original path in each `folder.json` (research option 4); (5) last resort: `msg-<8 hex of the identity hash>` for the message directory, flagged. Every step is deterministic; the budget and each decision are recorded in the metadata. **Implemented: steps 1-3. Not implemented: steps 4 and 5.** When the floors are not enough, the planner counts the entry as over budget (`plan_budget_exceeded`) instead of flattening; the PST fixture shows 4 such entries at a root of 152 units, and an export refuses such a plan (section 2).

**L5. How to shorten.** Cut at a code point boundary that is not inside a surrogate pair or combining sequence, never inside a grapheme cluster, then append `…` (U+2026, one unit).

**L6. Directory size.** FAT32 allows at most 65,534 entries per directory (verify); NTFS has no practical limit but Explorer slows with tens of thousands. Warn and report the maximum entries in any directory (`plan_max_entries_in_directory`; implemented). NTFS is recommended for the output.

**L7. Long paths: advice (Q7, delegated to Claude).** Default: obey the 259 budget; provide `--long-paths` as an explicit opt-in, off by default. Rationale: the archive's purpose is durability and portability. `\\?\` or registry-enabled long paths work for the tool that wrote them, but Explorer in older configurations, many archivers, sync clients, backup software, PowerShell 5, and other operating systems routinely fail on long paths, and a durable archive that cannot be copied is not durable. Do *not* apply a blanket "portable" relative cap by default (it would shorten names needlessly for people exporting near a drive root). Instead: (a) compute the budget from the real root; (b) record the longest relative path in the root `folder.json` and report `plan_max_relative_path_units`; (c) offer `--max-relative-path N` for someone exporting an archive meant to move to a longer root; (d) when names must be shortened, say so in counts and in metadata. Suggest, in documentation, exporting to a short path such as `D:\tsp`.

## 6. Uniqueness and duplicates

A directory is one namespace on Windows: a file and a directory cannot share a name, and names compare case-insensitively. A PST has separate namespaces (a folder and a message may both be called `Budget`; sibling folders may share a name). In this layout the namespaces that matter are: (a) a PST folder directory's children (subfolder directories and message directories, plus the reserved `folder.json`); (b) a message's `attachments/` (attachment files and embedded-message directories).

**U1. Collision key.** `casefold(NFC(final component name))`, computed on the **final** on-disk name within one of the namespaces above. Case folding is a conservative superset of NTFS behavior; NFC protects a later move to macOS. Verify: NTFS uses its own upcase table.

**U2. Natural names claim first.** Within a namespace, compute every item's natural name (sanitized, truncated). Non-colliding natural names are final. Only then are colliding items renamed, so a message genuinely called `Budget (2)` keeps its name and a generated suffix skips over it.

**U3. Deterministic order.** Items in a collision group are ordered by (kind, then message time ascending with missing times last, then the item's stable identifier ascending); folders rank before messages, so a folder keeps its bare name against a message with the same name. The first keeps the natural name; the k-th item gets suffix number k. For folders the order is the folder identifier (section 9). Order never depends on scan order, scheduling, or the clock. Message time is delivery time, then submit time, then a fixed value that sorts last. Because the suffix is zero-padded and follows this order, a plain text sort reproduces chronological order within a group.

**U4. Suffix scheme (decision A11).** One scheme for folders, messages, and attachments (a stem that ends in a space loses it before the suffix is added, so no double space appears):

- The first item in a group keeps the bare name. The k-th item (k >= 2) is named `<name> (kk)`, where `kk` is k zero-padded to `max(2, digits of the group size)`. A group of 30 uses ` (02)` to ` (30)`; a group of 1,500 uses ` (0002)` to ` (1500)`.
- The number is the item's *position* in the group (first item is 1, implicit), not a count of copies. The second item is ` (02)`, matching Windows's ` (2)`, and the number does not claim the item is a copy.
- The group width is per group, so a group that grows from 99 to 100 members renames its members once; groups of that size (for example thousands of `(no subject)` messages or automated alerts in one folder) are expected in real mailboxes, which is why the scheme does not assume at most 99.
- No ` - Copy` label exists. Whether two messages are content-identical is **metadata** (`content_sha256` and `identical_to` in `metadata.json`, the hash in the folder's `folder.json` index), not part of any name. Naming therefore depends only on names, times, and identifiers, never on hashing message content.

**U5. Suffix placement and length.** Suffixes are appended to the directory name (no extension to split). If a suffix would exceed the budget, shorten the stem (L4), never the suffix. Suffixes count toward L1 and L3 (6 units reserved; wider only for groups of 1,000 or more). The truncation of a colliding group's members happens before suffixing, so all members share one truncated stem.

**U6. Folders.** Sibling folders that collide (same name, case-only, or normalization differences) are ordered by identifier and suffixed by U4. The original name and identifier are recorded in `folder.json`.

**U7. Attachments.** Within one `attachments/`, colliding names are suffixed by U4 before the extension (`image001 (02).png`), in attachment-table order. An embedded-message directory and an attachment file compete in the same namespace.

**U8. Reserved keys.** `folder.json` and, under `<out>`, `.tsp-tmp` take part in U1.

**U9. Final check.** After planning, rebuild every namespace's collision keys and assert there are no duplicates. A violation is a bug, and the export refuses to start. (Implemented as `verify_plan`; the census reports `plan_gate_collisions`, 0 on both fixtures, which include 3 collision groups on the PST. An export refuses a plan with any gate violation.)

**Stability.** Re-exporting an unchanged PST gives the same names. If messages are added, later positions in a group can shift, and a group that crosses 99 or 999 members is renamed once; the stable identity in `metadata.json` is what lets a later tool match directories to messages.

## 7. Attachments (`attachments/`) and embedded messages

The accepted layout has none of the problems of a `<subject> - attachments` sibling folder: the literal `attachments/` name is 11 units, sits inside the message directory (so it cannot collide with a subfolder or another message), and needs no suffix logic. What remains:

1. **The attachment path is the longest path** (L3). A subject that fits as a message directory may not leave room for a long attachment name, so the reservation is computed from the longest attachment actually present.
2. **Embedded messages recurse.** A forwarded chain repeats `<subject>\attachments\` at every level. Proposal: a depth cap (default 3); beyond it, the embedded message is written, flagged, into the deepest legal level as a flat directory named `Embedded - <subject>`, and the flag records the original nesting. Also stop cycles (corrupt files).
3. **Created only when needed.** `attachments/` is created only if at least one file or embedded message directory is written. An attachment that cannot be extracted (PST attachment bytes, OLE objects) still appears in `metadata.json` with its status and reason. (M4c writes no attachments; it counts them and records `attachments_not_extracted`.)
4. **Inline attachments (decision Q6).** Signature logos and pasted pictures stay in `attachments/` and carry `inline: true` in `metadata.json`; the corpus has 23 content-ID attachments among 29, so many messages will have an `attachments/` holding only `image001.png`. Keeping them is the faithful choice, and the flag lets a reader or tool filter them.
5. **Attachment name hygiene** applies (N4, N6, N8, N10). Executable attachments may be quarantined by antivirus.

## 8. Export lifecycle

1. **Plan (pure).** Walk the source; apply N, L, and U rules; produce every directory and file with final names, lengths, and flags. No I/O. The same code serves `--dry-run`. (Implemented.)
2. **Gate.** Check plan invariants (U9, budgets) and print counts-only findings in the `--verify` style, under keys such as `plan_names_sanitized`, `plan_names_truncated`, `plan_collision_groups`, `plan_max_path_units`, `plan_budget_exceeded`, `plan_gate_violations`. (Implemented in `--dry-run`; an export stops here if a gate is violated.)
3. **Preflight against the target** and consent (section 2). (Implemented in M4c. The whole archive is rendered in memory first so existing files can be compared with what would be written; streaming is an M4h item.)
4. **Write** into a staging directory `<out>/.tsp-tmp/` (short fixed name so staging paths never exceed final paths) and move into place. **As built:** each file is written under a numeric name (`1`, `2`, ...) in `.tsp-tmp` and renamed to its final path; `std::fs::rename` replaces an existing file on Windows (observed on the owner's Windows setup when an edited file was restored). The root `folder.json` is first written as `incomplete` and replaced by the `complete` version last. Leftover numeric files from an interrupted run are removed at the start; any other entry in `.tsp-tmp` stops the export. The `--out` path must be short enough (about 239 units) that a staged path cannot exceed 259. Only files whose content differs are written. This is per-file atomic, not whole-archive atomic.
5. **Verify** by reading back and comparing with the plan; remove `.tsp-tmp`. (Implemented in M4c: every generated file is read back and compared, `export_readback_mismatches`; a mismatch is an error.)

File modification times: set to the message time when valid and representable (Windows cannot represent times before 1601, and PSTs can contain placeholders such as the year 4500), otherwise leave as written. Times are excluded from determinism comparisons. (Not implemented; files get the time they were written.)

## 9. Folder identity

- Every PST node has a 32-bit **NID**, unique within that PST. A folder's `PidTagRecordKey` and `PidTagEntryId` are *calculated properties derived from the NID* (MS-PST lists both with base tag `nid`). The EntryID also contains the store's 16-byte **Provider UID** (the store's own `PidTagRecordKey`).
- So a folder identifier is unique within one PST and distinguishable across different PSTs by the Provider UID, but it is not a free-standing GUID. Two byte-identical copies of a PST share all of it.
- `outlook-pst` v1.2.0 exposes, per folder, `FolderProperties::node_id()` (the NID), `display_name()`, and a computed `PidTagEntryId` (0x0FFF); it exposes no separate record key for folders. This comes from reading the crate source. **Run result (M4b-3, `tsp-tester.pst`):** a NID and an EntryID were both available for all 11 folders walked and none were unavailable (`folder_identity_nid_available=11`, `folder_identity_entry_id_available=11`, `folder_identity_unavailable=0`). Other producers are untested.

Use: (1) **ordering key** for U3/U6 (NID ascending); (2) **recorded in `folder.json`** (NID, EntryID hex, Provider UID, original display name, parent NID, original path); (3) **never in the visible name** except through the ` (n)` rule when siblings collide; (4) if the identifier cannot be obtained, fall back to (display name, position in the hierarchy table, content count) and flag `folder_identity_fallback`; that order is deterministic for a given file but not meaningful across files. (M4c records none of the identity fields yet; they arrive with the PST export and M4g.)

**Message identity** (the item the accepted ADR deferred): the directory name is for humans and is not the identity. Identity is the source identifier (for a PST message: NID plus the store's Provider UID), recorded in `metadata.json` together with `PidTagInternetMessageId` when present, and the content hash. Missing or duplicated Internet message IDs therefore cannot affect naming, which resolves the ADR's deferred question for folder naming. (The census confirms duplicates are common: 4 of 34 messages in the `.msg` directory and 20 of 60 in the PST share an Internet message ID with another message.)

## 10. Where PST and Windows differ (checklist)

| # | Difference | Handled by |
|---|---|---|
| 1 | Reserved characters, device names, trailing dots/spaces | N3, N4 |
| 2 | Case-insensitive, case-preserving names | U1 |
| 3 | Total path limit; component limit | L1-L4 |
| 4 | Duplicate names are legal in a PST | U2-U7 |
| 4a | **Thousands of same-subject items in one folder** (`(no subject)`, alerts) | U4 per-group width |
| 5 | Same-name sibling folders are possible | U6, section 9 |
| 6 | Unbounded folder depth | L4 flattening (not implemented) |
| 7 | Copies are independent objects | both exported with position suffixes; identity recorded in metadata |
| 8 | Unrestricted subject length and content | N3, L4, L5 |
| 9 | Attachment names missing, illegal, duplicated | N8, U7 |
| 10 | **Files and directories share one namespace on Windows; a PST separates them** | U1 (namespaces in section 6) |
| 11 | **Length is counted in UTF-16 units** | section 5 |
| 12 | **Normalization: NFC and NFD names look identical** | N2, U1 |
| 13 | **Invalid UTF-16 and code-page loss in ANSI PSTs** | N2 |
| 14 | **Invisible and bidi characters in subjects** | N3 |
| 15 | **Empty subject or folder name** | N8 |
| 16 | **The `--out` root and PST stem consume the budget** | L2, L7 |
| 17 | **Message directories are directories: the 248 rule, device names apply** | L2, N4 |
| 18 | **Fixed child names, suffixes, and staging consume the budget** | L3 |
| 19 | **Tool-owned names may collide with user folders** | N9, U8 |
| 20 | **Files per directory (FAT32 65,534)** | L6 |
| 21 | **File timestamps cannot always be represented** | section 8 |
| 22 | **Non-mail items (calendar, contacts, tasks, notes)** | section 12 |
| 23 | **Hidden/associated items, search folders, items outside the IPM subtree** | section 12 |
| 24 | **Sync clients add their own name and path limits** | document; census warns on long paths |

## 11. ADRs this implies (complete, do not supersede)

The accepted ADR [Deterministic Markdown archive](../../ADRs/deterministic-markdown-archive.md) keeps its status and layout. M4a-2 added ADRs that decide what it deferred; the archive ADR carries a cross-reference to them (a link, not a status change). Status as of 2026-10-04:

1. **Export naming, collisions, and path budgets** (sections 4-6) — [accepted](../../ADRs/export-naming-collisions-and-path-budgets.md).
2. **Message and folder identity and provenance** (section 9; fills the ADR's deferred identity item) — [accepted](../../ADRs/message-and-folder-identity-and-provenance.md).
3. **Output posture** (`--out`, `--dry-run`, consent, ownership by `folder.json`, stdout content-free; sections 2 and 8) — [accepted](../../ADRs/export-output-posture-consent-and-ownership.md).
4. **Body and formatting-loss policy** (M4e) — **not yet decided**; it waits for the HTML-to-Markdown comparison and the de-encapsulation work.
5. **`metadata.json` / `folder.json` file rules and schema evolution** (section 3.2) — [accepted](../../ADRs/archive-file-contract-and-schema-evolution.md). The field-level schemas are not frozen by it; they are frozen in M4g.

## 12. Scope of what is exported (decision Q5)

Everything reachable from the IPM subtree, including Deleted Items. Every item gets a message directory: mail (`IPM.Note*`) with a body in `message.md`; other classes (appointments, contacts, tasks, notes, reports, meeting requests) get `message.md` with a short metadata summary and status `partial: non-mail item` until they have a renderer. Skipped and counted, never silently: associated (hidden) items, search folders (virtual), and anything outside the IPM subtree (orphans, recoverable items). Counts appear in the root `folder.json` and in the stdout summary. (Not implemented for export yet; M4c handles `.msg` input only.)

## 13. Open questions

All of Q1-Q10 are decided (see the decisions in force in section 1 and the owner's answers recorded in `m4-plan.md`). One remains, and it no longer blocks naming:

- **Q11.** The exact definition of the content hash recorded in `metadata.json` and the `folder.json` index (M4g ADR). Proposed contents: class, subject, sender, recipients, sent and delivery times, all body variants, and attachment names and bytes; excluded: node ID, EntryID, creation and modification times, read flags. The naming census does not need it.
