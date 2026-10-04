---
status: "accepted"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Message and folder identity and provenance

*Status history: proposed 2026-10-04; accepted 2026-10-04 by the owner. Acceptance records the decision, not the work: part of it is not yet implemented (see Confirmation).*

## Context and Problem Statement

[Deterministic Markdown archive](deterministic-markdown-archive.md) deferred "message identity / provenance strategy (deterministic archive folder naming when `PidTagInternetMessageId` is missing or duplicated)". With names now settled ([Export naming, collisions, and path budgets](export-naming-collisions-and-path-budgets.md)), the remaining question is: what *identifies* an exported message or folder, so that a later tool (re-export, diff, merge, verification) can match an archive entry to its source item and to other entries, even though a directory name is derived from a mutable, non-unique subject?

## Decision Drivers

- A directory name is for people and may change (a suffix can renumber; rules may evolve); it cannot be the identity.
- The Internet message ID is not a usable key: in the fixtures 4 of 34 `.msg` messages and 20 of 60 PST messages share one with another message, and some messages have none.
- Identity must be obtainable for every item or have an explicit, deterministic fallback.
- Identity must not leak machine-specific values (absolute paths) into the archive, and must be identical across exports of the same input.
- Provenance (where an item came from) is part of the model: the source file's identity should be recorded, with the option to omit the hash.

## Considered Options

* **A. Record several identities in metadata, none in the name: a source-position identifier, the Internet message ID when present, and a content hash (chosen).**
* B. Use the Internet message ID as the identity.
* C. Use the subject (directory name) as the identity.
* D. Use a content hash as the only identity (and as the directory name).

## Decision Outcome

Chosen option: "A", because no single available identifier is both always present and unique, while the combination is, and recording them in metadata keeps names free of them.

1. **The directory name is never the identity.** Identity is recorded in `metadata.json` (messages) and `folder.json` (folders). Archive tools match entries by identity, not by name.
2. **Folder identity (PST).** The folder's node ID (unique within the PST) and its entry ID (derived from the node ID and the store's provider UID) are recorded, along with the original display name and original path. The node ID is the ordering key for sibling folders that collide. If neither is available, a deterministic fallback (display name, position in the hierarchy table, content count) is used and flagged `folder_identity_fallback`.
3. **Message identity (PST).** The message's node ID together with the store's provider UID. `PidTagInternetMessageId` is recorded when present, as supporting information, never as the key.
4. **Message identity (`.msg`).** The source file (its name, or its path relative to the input directory, never an absolute path), plus the Internet message ID when present. The source file's SHA-256 is recorded in the archive root by default (`--no-source-hash` omits it); for a directory input it is the hash of the sorted `<relative path>\t<file hash>` lines.
5. **Content identity** (a hash over a defined set of message content, and `identical_to` listing earlier messages with the same hash) is recorded in metadata only, never in names. Its exact definition is the one question still open (Q11) and is decided with the metadata work (M4g).
6. **Archive-level source identity** (used to decide whether an existing target belongs to this export) is the source's kind and name. It deliberately does not include the hash, so that re-exporting a source that has changed is allowed with consent.

### Consequences

* Good, because names stay readable and are free of identifiers, while every item still has at least one stable identity in metadata.
* Good, because a missing or duplicated Internet message ID cannot affect naming or matching.
* Bad, because identity is only as good as the source: a PST node ID is unique within one file, and byte-identical copies of a PST share all of it.
* Bad, because a `.msg` message has no identity of its own beyond its file name and the optional Internet message ID; two copies of the same message in different files are only linked through the content hash (once defined).

### Confirmation

* Evidence so far: on the PST fixture a node ID and an entry ID were both readable for all 11 folders walked and none were unavailable (`folder_identity_*` counts, M4b-3). Duplicate and missing Internet message IDs were counted (4 and 20 duplicates; 0 missing in both).
* Implemented in M4c (draft schemas): `.msg` provenance in `metadata.json` (`source_file`, `internet_message_id`) and the root `folder.json` (`source` with kind, name, size, optional SHA-256). Archive-level source identity (point 6) and its refusal rule are implemented and tested.
* **Not yet implemented:** recording folder identifiers in `folder.json` and PST message identifiers in `metadata.json` (waits for the PST export, M4i, and the schema work, M4g); the content hash and `identical_to` (Q11, M4g). Until then points 2, 3, and 5 are decisions without code.
* Not yet confirmed: that the node-ID-derived identifiers are present across other producers; the fallback ordering has never been exercised on real data.

## Pros and Cons of the Options

### A. Several identities in metadata

* Good, because it works when any one of them is missing.
* Bad, because it asks the metadata schema to carry several identifiers, and the content hash needs a careful definition.

### B. Internet message ID as the identity

* Good, because it is the identifier users and mail tools know.
* Bad, because it is missing or repeated in real data (see the counts above), including for legitimate distinct messages.

### C. Subject as the identity

* Good, because it needs nothing extra.
* Bad, because subjects are not unique, are sanitized and shortened for the name, and `(no subject)` is common.

### D. Content hash as identity and name

* Good, because identical content collapses to one identity.
* Bad, because it makes names unreadable (see option B of the naming ADR), requires hashing every message before naming, and any change to the hash definition would rename the whole archive.

## More Information

* Rules and the identity discussion: [`docs/plans/m4a-export-rules.md`](../docs/plans/m4a-export-rules.md) section 9.
* The schemas that carry these fields: [Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md).
* Revisit when: the PST export (M4i) shows an item without an obtainable identifier, or Q11 is decided.
