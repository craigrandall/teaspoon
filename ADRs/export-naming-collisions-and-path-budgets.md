---
status: "proposed"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Export naming, collisions, and path budgets

*Status history: proposed 2026-10-04 (drafted from the M4a rules, the M4b-2 implementation, and the M4b-3 / M4c results). Becomes accepted when the owner says so.*

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

1. **Sanitizing.** Normalize to NFC; remove the subject-prefix marker (U+0001 plus one character); replace `< > : " | ? *` with `_` and `/` `\` with `-`; delete control, bidi, and zero-width characters; turn any whitespace into a single space and trim; trim trailing dots and spaces; append `_` to a device name (also for directories); treat empty, `.`, and `..` as empty and use a fixed fallback (`(no subject)`, `(unnamed folder)`, `archive`, `attachment-<n>`). Attachment names that Explorer treats specially get a leading `_`. The replacement table is fixed permanently.
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
* A real export of the `.msg` corpus (M4c): 0 gate violations at a short output path, a stable tree hash across runs and output directories, and a longest written path of 131 units.
* Not yet confirmed: the PST at a short output path; the PST counts against the earlier PST diagnostic (unreconciled); adversarial names on real Outlook data (the fixture list in the M4 plan); whether NTFS's own case folding ever disagrees with the collision key (the key is a conservative superset by design, but this is unverified); rule L4 steps 4 and 5.

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
