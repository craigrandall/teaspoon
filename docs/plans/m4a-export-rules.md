# M4a draft — export naming, layout, and overwrite rules

Status: **proposed draft, nothing implemented or verified.** This is the working specification for the naming and layout decisions the M4 plan calls M4a. It is meant to be argued with, then frozen as ADRs after the naming census (M4b-3) has produced evidence from real fixtures. Every "verify" tag marks a claim that was taken from documentation or memory and has not been checked against this project's fixtures or Windows.

Inputs: the project owner's requirements (2026-09-30), the external research "Support for linting a PST, anticipating a Windows filesystem for message export" (2026-09-29, referred to below as *the research*), Microsoft's Win32 naming and MS-PST documentation, and the existing ADRs.

## 1. Requirements taken as given

1. Exporting a PST mirrors its folder tree as a directory tree.
2. Each message becomes a `.md` file whose name mirrors the message's subject.
3. Duplicate names are disambiguated in the Windows style (` - Copy`, ` - Copy (2)`, ...), with the differences between PST and Windows semantics fully accounted for up front, including path length.
4. A message that has attachments gets a folder named `<subject> - attachments`.
5. Sibling PST folders with identical names are told apart using the folders' own identifiers.
6. If an export could overwrite existing content, `tsp` warns and asks permission to continue, or exits.
7. Stdout stays content-free (counts only). Archive content goes only into the directory the user names.

## 2. What "CLI shape" means, and a proposal

"CLI shape" is how the user asks for an export on the command line, not what is written to disk. Today `tsp <input>` prints a content-free diagnostic and `tsp --verify <input>` compares two parsers. Export is a new behavior that writes message content, so it needs its own spelling, and the choice matters because existing usage and the stable output vocabulary must keep working.

Proposal:

```text
tsp <input>                          # unchanged: content-free diagnostic
tsp <input> --out <dir>              # export
tsp <input> --out <dir> --dry-run    # project the export, report counts, write nothing
tsp <input> --out <dir> --overwrite  # non-interactive consent to replace files this tool would generate
```

Reasons: `--out` cannot be confused with the diagnostic, `--dry-run` falls out of a pure planning phase (section 8), and consent can be given interactively or by flag. Alternatives (a `tsp export` subcommand) are equivalent in substance; the flag form matches the existing idiom.

Consent behavior (requirement 6):

- Preflight compares the plan with what is already in `<dir>` and reports **counts only** to stdout: files that would be created, files that already match, files that would be replaced, and files present that this export would not touch.
- Interactive terminal: print the counts and ask `Continue? [y/N]`. Anything but an explicit yes exits with code 2 and writes nothing.
- Not interactive (stdin is not a terminal): behave as "no" unless `--overwrite` was given.
- `tsp` only ever replaces files whose names its own plan generated. It never deletes files it did not create. A separate `--clean` (delete previously generated files that the new plan no longer produces) would require the archive's ownership manifest to be present and would need its own consent; propose deferring `--clean`.
- Writes go to a staging area and are moved into place, so a failed or refused run leaves the target as it was (see section 8).

## 3. Layout

Requirement 4 changes the layout the accepted ADR "Deterministic Markdown archive" chose (a folder per message containing `message.md`, `metadata.json`, and `attachments/`). This proposal replaces it, so it needs a superseding ADR (see section 11).

```text
<out>/
  <store name>/                       # see open question Q1
    Inbox/
      Budget review.md
      Budget review - attachments/
        Q3 plan.xlsx
        image001.png
      Budget review - Copy.md          # content-identical duplicate
      RE_ Budget review.md             # ':' replaced
      RE_ Budget review (2).md         # same sanitized name, different content
      Projects/
        ...
    Sent Items/
  .tsp/                               # tool-owned; see section 3.2
    manifest.json
```

### 3.1 What maps to what

| PST | Filesystem |
|---|---|
| Folder | Directory named from `PidTagDisplayName` |
| Message | `<subject>.md` in the folder's directory |
| Attachment (file) | File inside `<message stem> - attachments/` |
| Embedded message | `<subject>.md` inside the attachments folder, with its own ` - attachments` folder if it has attachments (recursive, depth-capped; section 7) |
| Folder / message identity | `.tsp/manifest.json` and message front matter (3.2) |

### 3.2 Where metadata lives (open question)

The research proposed YAML front matter in each `.md`. `serde_yaml` is reportedly archived and unmaintained, and YAML is harder to keep deterministic than JSON, so avoid it. Proposal: each message's `.md` starts with a single comment line carrying its stable identity (`<!-- tsp:id=... -->`), and everything else (property bag, recipients, status, original names, provenance) lives in `.tsp/messages/<id>.json`, with `.tsp/manifest.json` listing every message and folder with hashes. This keeps the visible tree clean, keeps metadata out of the path budget, and keeps the user tree grep-friendly. The trade-off is that a user who copies only the visible tree loses the metadata. Alternative: sidecar `<stem>.json` files next to each message (visible, but doubles the file count and consumes path budget). Decision for the project owner (Q3).

The name `.tsp` is reserved at the export root (rule N9).

## 4. Deriving and sanitizing names

Applied to folder names, message stems, and attachment names. All rules are pure functions of their inputs.

**N1. Source strings.** Folder: `PidTagDisplayName`. Message: `PidTagSubject` (the full subject, which per MS-OXCMSG is the subject prefix plus the normalized subject, so `RE: ` is kept as the owner asked). Attachment: `PidTagAttachLongFilename`, then `PidTagAttachFilename`, then a synthetic name (N8). Verify: some producers reportedly store control characters at the start of the subject to mark the prefix; the census must show whether the corpus has them (rule N3 strips them regardless).

**N2. Unicode form.** Normalize to NFC. Unpaired surrogates in PST UTF-16 data cannot be represented in a Rust string; decode with replacement and flag the item (`name_lossy_decode`). ANSI PST strings are decoded through the code page chain already built for `PT_STRING8`; an unsupported code page yields the synthetic name (N8) and a flag.

**N3. Characters removed or replaced.**

| Class | Action |
|---|---|
| `< > : " / \ | ? *` | replace with a visually close safe character (proposal below), never delete |
| C0 controls U+0000-U+001F, DEL U+007F, C1 controls U+0080-U+009F | delete, flag |
| Line/paragraph separators U+2028/2029, bidi controls U+202A-202E and U+2066-2069, zero-width characters U+200B-200D and U+FEFF | delete, flag |
| Leading/trailing Unicode whitespace (including U+00A0 and U+3000) | trim |
| Trailing `.` or space (after all other steps) | trim repeatedly |
| Runs of whitespace | collapse to one space |

Suggested replacements (owner to approve): `:` becomes `-` when it is followed by a space (`RE: x` becomes `RE- x`, which looks odd); the research suggested `_`. Proposal: replace every reserved character with `_`, except `/` and `\`, which become `-`. Whichever is chosen is fixed forever, because changing it renames every exported file.

**N4. Reserved device names.** Compare the part of the component before the first `.` (after trimming), case-insensitively, against `CON PRN AUX NUL COM0-COM9 LPT0-LPT9` and the superscript variants, plus `CONIN$ CONOUT$`. On a match, append `_` to that part (`NUL` becomes `NUL_`). This applies to folders, message stems (because `NUL.md` is the device), and attachment names. Verify the exact set against current Microsoft documentation; the conservative superset costs nothing.

**N5. Dot names.** A component that becomes `.` or `..` is treated as empty (N8).

**N6. Explorer-special and tool-special names.** Attachment names equal (case-insensitively) to `desktop.ini`, `thumbs.db`, `autorun.inf`, or beginning with `~$` get a leading `_`. Verify the list. Executable attachment types written to disk may be quarantined by antivirus; that is behavior to document, not a naming rule.

**N7. Idempotence.** `sanitize(sanitize(x)) == sanitize(x)`. Tested by property tests.

**N8. Empty or unusable names.** Empty after sanitizing: folder `(unnamed folder)`, message `(no subject)` (matches what Outlook displays), attachment `attachment-<n>` where `<n>` is the 1-based position in the attachment table, with an extension from the MIME type only if one is recorded. Never omit an item.

**N9. Reserved for the tool.** At the export root, a PST folder named `.tsp` (any case) is renamed by the uniqueness rules as if it collided with a sibling.

**N10. Extensions.** Message files always end in `.md`. A subject that already ends in `.md` is not special. For attachments, the extension is the text after the last `.` if that text is 1-16 characters with no spaces; otherwise the attachment has no extension. The extension is never truncated (L3).

## 5. Length and path budgets

**Measurement.** Windows limits count UTF-16 code units, not characters and not bytes. An emoji is 2 units. Every length in these rules is UTF-16 code units. Verify with tests using astral-plane characters.

**L1. Component limit.** 255 units per component on NTFS, FAT32 and exFAT. After all suffixes are added, a component must be at most 255; the working target is lower (L3).

**L2. Path limit.** Default budget: the full path, including drive, separators, and the terminating null, must be under 260. That leaves 259 units for the full path of the deepest file or directory. Two documented wrinkles: (a) `CreateDirectory` reportedly enforces a lower limit of 248 for directory paths (so an 8.3 name can still fit); treat directory paths as at most 247. (b) The user's `--out` path counts, so the relative budget is `259 - len(absolute out root) - 1`. Both verify against Microsoft documentation.

Long-path support (registry `LongPathsEnabled` plus manifest, or the `\\?\` prefix) would allow more, but archivers, sync clients, older tools, and other operating systems still fail on long paths. Default is to obey the 259 budget; a `--long-paths` flag can lift it deliberately. Rust's standard library is believed to handle long Windows paths by itself (verify); the risk is downstream tools, not `tsp`.

**L3. Reserved room.** Truncation is computed against the *worst-case* child, not the item itself:

- a message stem reserves ` - Copy (99)` (12 units) so a later duplicate still fits, plus `.md` (3) for the file;
- a message with attachments reserves ` - attachments` (14 units), one separator, and the longest attachment name it will actually contain (capped, default 100 units including extension), plus ` - Copy (99)` for that name;
- a folder reserves one separator plus the shortest legal child.
- the atomic-write staging path (section 8) must not exceed the final path, so staging uses a short fixed prefix.

**L4. Truncation order.** When the projected path exceeds the budget: (1) shorten message stems, longest first, down to a floor of 24 units; (2) shorten attachment stems down to a floor of 16; (3) shorten folder names down to a floor of 16, deepest first; (4) flatten: fold the remaining over-budget subtree into its nearest fitting ancestor as `Parent - Child - Grandchild` (research Option 4) and record the original path in the manifest; (5) if even that fails, fall back to `msg-<8 hex of the identity hash>.md` (N8-style) and flag. Every step is deterministic given the same input and the same budget; the budget and each decision are recorded in the manifest.

**L5. How to shorten.** Cut at a word or code point boundary that is not inside a surrogate pair or a combining sequence, then append `…` (U+2026, 1 unit). Never cut inside a grapheme cluster.

**L6. Directory size.** FAT32 allows at most 65,534 entries per directory (verify); NTFS has no practical limit but Explorer degrades with tens of thousands. A warning is enough: report the maximum entries in any directory. Recommend NTFS for the output.

## 6. Uniqueness and duplicates

A directory is one namespace on Windows: a file and a folder cannot share a name, and names are compared case-insensitively. A PST has separate namespaces (a folder and a message may both be called `Budget`), so collisions the PST never had appear on export.

**U1. Collision key.** `casefold(NFC(final component name))`, computed on the **final** on-disk name including extension (`Budget.md`, not `Budget`), across the union of everything that will exist in one directory: subfolders, message files, attachments folders. Case folding is a conservative superset of NTFS behavior; NFC keys also protect a later move to macOS. Verify: NTFS uses its own upcase table.

**U2. Natural names claim first.** For each directory, first compute every item's natural name (sanitized and truncated). Natural names that do not collide are final. Only then are colliding items renamed. This guarantees that a message really called `Budget (2)` keeps its name and a generated suffix skips over it.

**U3. Deterministic order.** Items in a collision group are ordered by (message time ascending, then the item's stable identifier ascending). The first keeps the natural name. For folders the order is the folder's identifier (section 6.1). Order never depends on scan order, thread scheduling, or the clock. Message time is the delivery time, falling back to submit time, then to a fixed value that sorts last.

**U4. Suffix scheme (owner's Windows-style convention, with one refinement).**

- Later items in a group whose **content is identical** to an earlier one (same content hash, defined in M4g) get ` - Copy`, ` - Copy (2)`, ` - Copy (3)`, counted within that content-identical set. These really are copies.
- Later items that share a name but **differ in content** get ` (2)`, ` (3)`, ... counted within the name group.

The refinement is a proposal for the owner. In a mailbox, same-subject messages in one folder are usually different messages (twelve replies titled `RE: Meeting`), not copies. Labeling them ` - Copy` would tell a future reader they are redundant, and in an archive that could lead to deleting evidence. Windows itself uses ` - Copy` only for genuine copy operations and ` (2)` for name conflicts. If the owner prefers ` - Copy` for everything, the mechanism is identical and only the string changes.

**U5. Suffix placement and length.** The suffix goes before the extension and counts against L1/L3. If adding the suffix would exceed the budget, shorten the stem (L4), never the suffix.

**U6. Attachments folder follows the message.** The attachments folder is named `<final message stem> - attachments`, so `Budget - Copy.md` pairs with `Budget - Copy - attachments`. The attachments folder takes part in U1 with the message's directory, and its own name is checked for collisions (a PST subfolder named `Budget - attachments` is renamed by U4, folders using the ` (2)` form).

**U7. Folder duplicates.** Sibling folders that collide (identical names, case-only differences, or normalization differences) are ordered by identifier; the first keeps the name and the rest get ` (2)`, ` (3)`. Folders are never ` - Copy`: two folders with the same name are not copies of each other, and the PST may hold different content in each. The original name and identifier are recorded in the manifest.

**U8. Attachment duplicates.** Within one attachments folder, colliding names get ` (2)` before the extension (`image001 (2).png`), in attachment-table order. Content-identical attachments still get ` (n)`, not ` - Copy`, because inline images with the same name and different content ids are common.

**U9. Verification of uniqueness.** After planning, a final pass rebuilds every directory's collision keys and asserts there are no duplicates. A violation is a bug, and the export refuses to start.

**Idempotence and stability.** Re-exporting the same unchanged PST produces the same names. If messages are added, the ordinals can shift for later items in a group. That is inherent to ordinal suffixes (the research's objection to them) and is accepted; the stable identity in `.tsp` is what lets a later tool match files to messages regardless of suffix.

## 7. Attachments folder: feasibility critique

Requirement 4 (`<subject> - attachments`) is feasible on Windows with these consequences that the requirement does not state:

1. **The attachment path is the longest path, not the message path.** `Dir\Subject.md` is `P + S + 3` units. An attachment is `Dir\Subject - attachments\name.ext`, which is `P + S + 14 + 1 + A`. The subject budget must therefore be computed from the attachment path (L3). A subject that fits as a message file can fail as an attachment parent.
2. **Cost of the suffix itself is small.** 14 units out of a 255 component and a 259 total path. It is not the problem. Nesting is.
3. **Embedded messages recurse.** A forwarded chain (message with an attached message with an attached message) repeats stem + ` - attachments` at every level, quickly consuming the budget. Proposal: a depth cap (default 3) and, beyond it, write the embedded message flattened into the deepest legal folder with a flag. Also stop recursion cycles (an embedded message cannot legitimately contain itself, but corrupt files exist).
4. **File and folder share a namespace.** `Subject - attachments` can collide with a PST subfolder or another message with that exact name (U6).
5. **Inline images inflate the count.** Signature logos and pasted images are attachments with a content id. The corpus has 23 content-id attachments among 29; many messages would get an attachments folder containing only `image001.png`. Options: (a) keep them all (simplest, faithful); (b) put inline attachments in an `inline` subfolder (extra depth); (c) keep them and mark them `inline: true` in the manifest. Recommend (a) with (c), and let the owner decide (Q6).
6. **Sorting.** Explorer sorts folders above files by default, so `Subject.md` and `Subject - attachments` are not adjacent. Cosmetic.
7. **Only when present.** The folder is created only for messages with at least one attachment. An attachment that could not be extracted (PST attachment bytes, embedded OLE) still appears in the manifest, and the folder is created only if at least one file is written; otherwise the message's metadata carries the flag.
8. **Attachment name hygiene** applies (N4, N6, N10), and executable attachments may be quarantined by antivirus when written.

## 8. Export lifecycle

1. **Plan (pure).** Walk the source, apply N/L/U rules, produce a plan: every directory and file with final names, lengths, and flags. No I/O. This is the same code the `--dry-run` uses.
2. **Gate.** Check plan invariants (U9, budgets). Print counts-only findings, in the `--verify` style, under keys such as `plan_names_sanitized`, `plan_names_truncated`, `plan_collision_groups`, `plan_max_path_units`, `plan_budget_exceeded`.
3. **Preflight against the target** (section 2), ask consent.
4. **Write** into a staging directory under `<out>/.tsp/staging-<short>/` and move into place. On the same volume, rename is cheap and atomic per file; replacing an existing file needs the Windows replace-on-rename call (verify what Rust's `std::fs::rename` does on Windows when the target exists).
5. **Verify** by reading back and comparing to the plan.

File modification times: set to the message time when it is valid and representable (Windows cannot represent times before 1601, and PSTs can contain placeholders such as the year 4500), otherwise leave as written. Times are excluded from determinism comparisons.

## 9. Folder identity (the "GUID" question)

The research and the owner both note that PST folders can share a name. What identifies a folder:

- Every PST node has a 32-bit **NID**, unique within that PST. A folder's `PidTagRecordKey` and `PidTagEntryId` are *calculated properties derived from the NID* (MS-PST folder objects table lists both with base tag `nid`). The EntryID also contains the store's 16-byte **Provider UID** (the store's own `PidTagRecordKey`).
- So the folder identifier is unique within one PST, and globally distinguishable across different PSTs by the Provider UID, but it is not a free-standing GUID assigned to the folder. Two byte-identical copies of a PST share all of it.
- Whether `outlook-pst` v1.2.0 exposes the NID or these properties for a folder has not been checked; that is a spike in M4b-3.

Proposed use:

1. **Ordering key** for U3/U7: sort colliding sibling folders by NID (ascending). Deterministic and independent of display order.
2. **Recorded in the manifest** (NID, EntryID hex, Provider UID, original display name, parent NID, original path), so a renamed directory can be traced to its folder.
3. **Never in the visible name** except through the ` (n)` rule, and only when siblings collide.
4. If the exposed identifier cannot be obtained, fall back to (display name, index in the hierarchy table, content count) as the ordering key and flag `folder_identity_fallback`. That ordering is deterministic for a given file but not meaningful across files.

## 10. Where PST and Windows differ (checklist)

Stated by the owner or the research, plus items neither mentioned:

| # | Difference | Handled by |
|---|---|---|
| 1 | Reserved characters, device names, trailing dots/spaces | N3, N4 |
| 2 | Case-insensitive, case-preserving names | U1 |
| 3 | Total path limit; component limit | L1-L4 |
| 4 | Duplicate names are legal in a PST | U2-U8 |
| 5 | Same-name sibling folders are possible | U7, section 9 |
| 6 | Unbounded folder depth | L4 flattening |
| 7 | Copies are independent objects | faithful mirror (both exported); ` - Copy` for identical content |
| 8 | Unrestricted subject length and content | N3, L4, L5 |
| 9 | Attachment names: missing, illegal, duplicated | N8, U8 |
| 10 | **Files and folders share one namespace on Windows; a PST separates them** | U1 (union) |
| 11 | **Length is counted in UTF-16 units** | section 5 |
| 12 | **Normalization: NFC/NFD names look identical** | N2, U1 |
| 13 | **Invalid UTF-16 and code-page loss in ANSI PSTs** | N2 |
| 14 | **Invisible/bidi characters in subjects** | N3 |
| 15 | **Empty subject or folder name** | N8 |
| 16 | **The `--out` root consumes the budget** | L2 |
| 17 | **Suffixes, ` - attachments`, and staging paths consume the budget** | L3 |
| 18 | **Tool-owned names (`.tsp`) may collide with user folders** | N9 |
| 19 | **Files per directory (FAT32 65,534)** | L6 |
| 20 | **File timestamps cannot always be represented** | section 8 |
| 21 | **Non-mail items (calendar, contacts, tasks, notes)** | section 12 |
| 22 | **Hidden/associated items, search folders, items outside the IPM subtree** | section 12 |
| 23 | **Sync clients (OneDrive and similar) add their own name and path limits** | document; census warns on very long paths |

## 11. ADRs this implies

1. **Export layout and naming** (supersedes the layout part of "Deterministic Markdown archive"; keeps its determinism and Markdown-as-projection decisions).
2. **Message and folder identity, duplicate policy** (mirror faithfully; U-rules).
3. **Output posture** (`--out`, `--dry-run`, consent, `.tsp` ownership, stdout content-free).
4. **Body and formatting-loss policy** (M4e).
5. **Metadata and per-item status schema** (M4g).

## 12. Scope of what is exported

Proposed default, for the owner to confirm (Q5): export everything reachable from the IPM subtree, including Deleted Items. Every item gets a `.md`: mail (`IPM.Note*`) with body; other classes (appointments, contacts, tasks, notes, reports, meeting requests) get a `.md` with a metadata summary and the status `partial: non-mail item` until they have a renderer. Skipped and counted, never silently: associated (hidden) items, search folders (virtual), and anything outside the IPM subtree (orphans, recoverable items). Counts appear in the manifest and in the stdout summary.

## 13. Open questions (owner)

- **Q1.** Export root naming: `<out>/<store display name>/...`, `<out>/<PST file stem>/...`, or the IPM children directly under `<out>`. Proposal: the PST file stem, sanitized, so several PSTs can share an output directory.
- **Q2.** The suffix scheme in U4 (content-identical ` - Copy`, otherwise ` (n)`) versus ` - Copy` for all.
- **Q3.** Where metadata lives (section 3.2).
- **Q4.** Replacement characters in N3.
- **Q5.** Scope in section 12.
- **Q6.** Inline attachments (section 7, item 5).
- **Q7.** Whether to allow `--long-paths`, and the default budget (259 versus a portable relative cap such as 200 so the archive can move to a longer root).
- **Q8.** Whether folders should carry a small visible marker file; proposal: no.
