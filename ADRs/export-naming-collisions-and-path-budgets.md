---
status: "accepted"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Export naming, collisions, and path budgets

*Status history: proposed 2026-10-04 (drafted from the M4a rules, the M4b-2 implementation, and the M4b-3 / M4c results); accepted 2026-10-04 by the owner. Acceptance records the decision; the items listed under Confirmation as not yet confirmed remain open. Amended 2026-11-09 (release v0.1.41, accepted by the owner): `~` is replaced by `_` in every name, and the determinism claim is qualified by the output root's length (see the amendment below).*

## Context and Problem Statement

[Deterministic Markdown archive](deterministic-markdown-archive.md) fixed the layout (one folder per message, mirroring the source's folder tree) and explicitly left filename sanitization and collision handling undesigned. Outlook allows what Windows does not: subjects and folder names with reserved characters, device names (`CON`, `NUL`), trailing dots and spaces, empty names, arbitrary length, and any number of siblings with the same name, in a namespace that is case-sensitive in the PST but case-insensitive on NTFS. Windows also counts length in UTF-16 code units, limits a component to 255 and (in practice) a path to 259, and limits directory paths to about 247. In this layout every message is a *directory*, so these limits bind harder than for a file-per-message layout.

How should every source name become a directory or file name so that the same input always yields the same names, nothing is lost silently, and no path is unusable on Windows?

## Decision Drivers

- Re-export stability: names are the visible part of an archive; changing the rules renames every directory.
- Determinism: names must not depend on scan order, scheduling, the clock, or the machine.
- Windows safety, up front: no export should fail halfway because of a name or path length.
- Readability: a person should recognize a message by its directory name.
- Loss is explicit: every change to a name is recorded, and a name that cannot be made to fit is reported, not hidden.
- Naming should not depend on hashing message content (so the naming census and the pure planner never read bodies).

## Considered Options

* **A. Subject-derived names, sanitized, with a uniform position suffix, planned as a whole against a path budget (chosen).**
* B. Opaque identifier names (node ID, or a hash of the message), with the subject only in metadata.
* C. Subject plus a short identifier on every name (for example `Budget review [3FA2]`).
* D. Date-prefixed names (`2026-03-04 Budget review`).
* E. Long-path (`\\?\`) writing to avoid shortening.

## Decision Outcome

Chosen option: "A", because it is the only option that keeps names readable *and* deterministic *and* free of identifiers in the common case; the cost (suffix numbers shift when messages are added; long roots may be refused) is bounded and visible.

The rules, in short (the full text, with rule numbers N1-N10, L1-L7, U1-U9, is in [`docs/plans/m4a-export-rules.md`](../docs/plans/m4a-export-rules.md) sections 4-6):

1. **Sanitizing.** Normalize to NFC; remove the subject-prefix marker (U+0001 plus one character); replace `< > : " | ? *` with `_` and `/` `\` with `-`; delete control, bidi, and zero-width characters; turn any whitespace into a single space and trim; trim trailing dots and spaces; append `_` to a device name (also for directories); treat empty, `.`, and `..` as empty and use a fixed fallback (`(no subject)`, `(unnamed folder)`, `archive`, `attachment-<n>`). Attachment names that Explorer treats specially get a leading `_`. The replacement table is fixed permanently, with one amendment (2026-11-09): `~` is replaced with `_` (see "Amendment, 2026-11-09" below).
2. **Length is counted in UTF-16 code units.** At most 255 per component; the default path budget is 259 units for every file and 247 for every directory, computed from the real absolute output root. Shortening cuts at a character boundary and ends with `…`.
3. **Worst-case child reservation.** A message directory's name is shortened using the longest path that will appear inside it (`metadata.json`, and the longest attachment name when there are attachments), plus room for its duplicate suffix. Attachment stems shrink first (to a floor of 16), then message names (floor 24), then folder names (floor 16).
4. **Uniqueness** is judged per directory namespace on `casefold(NFC(final name))`. Names that do not collide claim their natural name first; colliding items are ordered by (folders before messages, then message time with missing last, then stable identifier) and the first keeps the bare name, the k-th gets ` (kk)` where `kk` is k zero-padded to at least two digits and widened per group. There is no ` - Copy`: whether two messages are identical is metadata, never part of a name.
5. **The plan is made before anything is written and is checked by an independent verifier** (collisions, over-budget paths, invalid names, recorded lengths). An export refuses a plan that breaks any gate and writes nothing; the user chooses a shorter output path.
6. **Not decided here:** flattening over-budget folder chains and the hash-based last-resort name (rule L4 steps 4 and 5) are not implemented; until they are, an entry that cannot fit is reported over budget instead of renamed.

### Consequences

* Good, because names are readable, reproducible, and recorded: each `folder.json` and `metadata.json` keeps the original name and what was done to it.
* Good, because path problems surface as counts before any write, and the same planner serves `--dry-run` and export.
* Bad, because a suffix is a *position*: adding a message to a group can renumber later members, and a group crossing 99 or 999 members is renamed once. The stable identity lives in metadata (see [Message and folder identity and provenance](message-and-folder-identity-and-provenance.md)).
* Bad, because a long output root can make a legitimate mailbox unexportable until the user shortens it (observed: 4 of 154 PST entries at a root of 152 units).
* Bad, because directory-per-message makes the 247-unit directory limit the binding constraint, so long subjects are truncated often on long roots.

### Confirmation

* Pure code with unit and property tests (M4b-2): sanitizing is idempotent, shortening never exceeds its limit, assignment is unique, deterministic, and independent of input order, and suffix widths are tested up to groups of 1,500.
* The `--dry-run` census over the real `.msg` directory and the PST fixture (M4b-3): 7 names sanitized and 7 truncated in the `.msg` directory; 16 sanitized and 34 truncated, 3 collision groups (largest 16) and `plan_gate_collisions=0` in the PST. Root length plus longest relative path equals the longest path in both.
* A real export of the `.msg` corpus (M4c): 0 gate violations at a short output path, a stable tree hash across runs and across two output directories (at short roots, where the path budget cut no name: at a root of 17 units the longest path was 198 of 259; see the root-length note in the amendment below), and a longest written path of 131 units.
* The first export that writes attachment files (M4f-1, `v0.1.34`, synthetic): two attachments both named `image001.png` and one unnamed attachment each land as a file under their own, distinct planned name, with the bytes of the attachment each name was planned for, and a second export of the same message writes nothing. Not run on the real corpus: there the planned attachment names have been checked only by `--dry-run` (34 attachment files, 0 gate violations at a short output path, M4c).
* Since confirmed: the PST at short output paths (planned at roots of 14 to 28 units with 0 gate violations and a longest path of 253; exported at `C:\o.tester` on 2026-11-09 with 0 read-back mismatches and 253 as the longest path on disk) and the PST counts (the diagnostic, the dry run, and the spike agree on 60 messages and 84 attachments).
* Not yet confirmed: adversarial names on real Outlook data (the fixture list in the M4 plan); whether NTFS's own case folding ever disagrees with the collision key (the key is a conservative superset by design, but this is unverified); rule L4 steps 4 and 5.

## Amendment, 2026-11-09 (release v0.1.41): `~` is replaced by `_`

Proposed after the first export of the PST tester lost two files, and accepted by the owner on 2026-11-09. It adds one entry to the replacement table that Decision Outcome item 1 calls fixed, so it is recorded here and not only in the rules.

### Context

The tester export planned 269 files and wrote them all, but two attachment files were missing at read-back (`export_readback_mismatches=2`), and a second export into the same directory created them. Both were the numbered sibling of a name that contains `~1`: `Screen~1 (02).jpg` beside `Screen~1.jpg`, and the same shape with `202301~1`. Windows gives each long name a generated 8.3 short name of the form `SCREEN~1.JPG`, and a name that is itself a valid short name can be looked up through another file's generated one. The likely sequence: the (02) file was staged first and took the short name `SCREEN~1.JPG`; the rename of the bare `Screen~1.jpg` then resolved to that file and replaced it. This fits what was observed (which file was lost, the creation order, the effect of the fix). It has not been shown by an experiment (`dir /x`, `fsutil 8dot3name query`).

### Decision

Rule N3 replaces `~` with `_` and flags the name `tilde_replaced`, in every name the tool writes: folders, message directories, attachment files, and embedded-message directories, from both PST and `.msg` input. The component check in the plan verifier rejects a name that contains `~`, so a regression fails the gates. The original name stays in the `directory` record of the metadata, beside the new adjustment label. No name that reaches the disk contains `~`, and every generated short name does, so no name can equal another file's short name.

### Considered alternatives

* **Replace `~` in every name (chosen).** One rule for every kind of name and both inputs; it needs no knowledge of how Windows generates short names.
* Replace `~` only in attachment names. Not recommended: a message or folder directory can be hit the same way (`Docs~1` beside `Docs~1 (02)`), and a rule that depends on the kind of name is one more thing to get wrong.
* Replace `~` only in names shaped like a short name (up to eight characters, `~`, digits). Not recommended: deciding when it applies needs Windows' short-name rules, which differ by volume and settings, and a wrong guess brings the loss back.
* Keep the names and repair afterwards (read-back already detects the loss; re-write or rename the missing file). Not recommended: read-back cannot say which file replaced which, and the repair would rename entries after the plan was verified.
* Turn off short-name generation on the volume (`fsutil 8dot3name`) or write through `\\?\` paths. Not recommended: the first changes the user's machine, needs administrator rights, and does not protect an archive later copied to another volume; the second does not stop short-name lookup.
* Do nothing and document it. Not recommended: ordinary Outlook attachment names (`image~1.png`) would lose files silently.

### Consequences

* Good: removes the whole class of collision on every volume, without relying on Windows internals.
* Good: the change is flagged and counted (`plan_names_sanitized` includes it), and the original name is kept in the metadata.
* Bad: a name changes where it would have been harmless (a subject `~~~ Sale ~~~` becomes `___ Sale ___`), so a draft archive made before v0.1.41 differs for such names.
* Bad: rule N6's `~$` test (Word lock files such as `~$Budget.xlsx`) can no longer match in the planner, because sanitizing runs first. The name is exported as `_$Budget.xlsx` with the flag `tilde_replaced`, where before it was `_~$Budget.xlsx` with `special_name_protected`. The protection is the same in effect; the predicate stays for any caller that skips sanitizing.
* Neutral: a collision the rule creates (`a~b` and `a_b` in one folder) is resolved by the existing suffix rule U4.

### Confirmation

* Same fixture, same build apart from this change: the first export had 2 read-back mismatches and a repeat export wrote the 2 files; afterwards a fresh export of the same PST has 0 mismatches, a repeat export writes nothing, and the tree hash is stable (`53ab025c52dd5a9383ae0f08fe4494e1eee8baba73c7e0cd214beca3a4f386e5`).
* The rule fired in 2 `metadata.json` files of the tester export (they contain `tilde_replaced`) and in none of the `.msg` corpus export (counts-only check, 2026-11-09), so the `.msg` archive differs from v0.1.40's only by the version string recorded in the root marker (consistent with identical counts; not shown by building the v0.1.40 sources at 0.1.41).
* Tests: the names of the bug through `sanitize_component` and collision assignment, `is_valid_component`, idempotence with `~` in the property-test generator, and the `~$` consequence.
* Not confirmed: the short-name mechanism itself; other file systems (FAT, exFAT, network shares); the large PST (`crkb.pst` is expected to hold many names like `image~1.png` and has not been exported).

### Root length and reproducibility

The path budget (item 2) is computed from the real output root, so names close to the budget depend on the root's length. Two exports of the same input are byte-identical when the output roots have the same length, and for any roots at which no name is cut by the budget (as for the `.msg` corpus). Otherwise names, `adjustments`, `planned_longest_relative_path_units`, and so the tree hash can differ. Observed on `tsp-tester.pst` (2026-11-09): a root of 22 units gave a longest relative path of 231 and a root of 23 gave 230, with different tree hashes. This is by design: shortening against the real root is what keeps every path creatable. When the 259-unit limit is what cuts a name, the cut name leaves the 6-unit duplicate-suffix reserve unused, so the longest path in the plan is 253 (read from `plan.rs`; 253 in every PST plan and on disk at `C:\o.tester`). The archive-file-contract ADR carries the same qualification.

## Pros and Cons of the Options

### A. Subject-derived names with position suffix and a whole-plan budget

* Good, because readable, deterministic, and visible in a file browser.
* Good, because the plan is pure and checkable before any I/O.
* Bad, because position suffixes are not stable under additions, and long roots can be refused.

### B. Opaque identifier names

* Good, because stable, short, and collision-free.
* Bad, because the archive becomes unbrowsable; the layout ADR's goal is a human-browsable archive.
* Bad, because identifiers differ per producer and are not always available (the Internet message ID is missing or repeated in practice: 4 of 34 and 20 of 60 messages share one in the fixtures).

### C. Subject plus a short identifier on every name

* Good, because no collisions to resolve and stable across additions.
* Bad, because every name carries noise, the identifier consumes budget in every path, and a short identifier can still collide.

### D. Date-prefixed names

* Good, because directory listings sort chronologically.
* Bad, because it costs 11 units on every name and still needs a collision rule; sorting by date is better left to metadata and tools.

### E. Long paths

* Good, because it avoids shortening.
* Bad, because a durable archive that Explorer, archivers, sync clients, and backup tools cannot copy is not durable. It remains an explicit opt-in (`--long-paths`, not implemented) rather than the default.

## More Information

* Rules, rationale, and the Windows checklist: [`docs/plans/m4a-export-rules.md`](../docs/plans/m4a-export-rules.md).
* Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md).
* Revisit when: flattening is implemented, a short-root PST run shows a gate failure, or an adversarial fixture shows a name this table mishandles.
