---
status: "proposed"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Archive file contract and schema evolution

*Status history: proposed 2026-10-04. Becomes accepted when the owner says so. This ADR decides the rules every archive file must follow and how the formats may change; it does **not** freeze the field-level schemas, which stay drafts (`0.1-draft`) until the metadata stage (M4g) has settled what they must carry.*

## Context and Problem Statement

[Deterministic Markdown archive](deterministic-markdown-archive.md) says `metadata.json` carries whatever the normalized model captured and that the same input must produce the same archive. M4c wrote the first real `message.md`, `metadata.json`, and `folder.json` files. Archives are meant to outlive the tool that wrote them, so later tools (and later versions of `tsp`) will read them. What rules must every file obey so that archives stay byte-reproducible and readable by future code, and how may the formats change without silently breaking old archives?

## Decision Drivers

- Reproducibility: two exports of the same input must be byte-identical, and a comparison should be one hash.
- Portability: nothing in the archive may depend on the machine, the user, or the time of export.
- Longevity: a reader must be able to tell which format version wrote a file.
- Loss is explicit: what was not extracted must be recorded in the file, not implied by its absence.
- Honesty about maturity: formats not yet proven should say so.

## Considered Options

* **A. A small set of binding file rules now, a `schema_version` in every JSON file, an additive-evolution policy, and field-level schemas frozen only when proven (chosen).**
* B. Freeze the full field-level schemas now.
* C. No versioning; the tool defines the format and readers follow the tool.
* D. Put metadata in YAML front matter inside `message.md` instead of separate JSON files.

## Decision Outcome

Chosen option: "A", because the stable part (determinism rules, versioning, loss recording) is known and worth fixing now, while the field list depends on envelope, attachment, body, and identity work that has not been done; freezing it early would force a breaking change or a wrong schema.

**File rules (binding for every file `tsp` writes into an archive):**

1. **Encoding.** UTF-8 without a byte-order mark, LF line endings, a trailing newline.
2. **JSON.** Pretty-printed with two-space indentation; object fields in a fixed, documented order; no unordered maps (so output cannot vary by hash seed or platform); numbers as JSON integers or strings, never floating point for identifiers or times; `null` for a known-absent value and omission only where the schema says so.
3. **No machine-specific values.** No absolute paths, user names, host names, or process IDs. Source files are referred to by their name or by a path relative to the input directory with `/` separators.
4. **No export-time values.** No export timestamps, durations, or run IDs. A time in a file is a time from the source. (File modification times are not part of the contract.)
5. **Every JSON file has `schema_version`.** Today's value is `0.1-draft`, meaning "may change without notice". A numbered version (`1`, `2`, ...) is assigned only when a schema is frozen.
6. **Status and loss are recorded.** Each message carries a status (`complete`, `partial`, or `failed`) and a list of reasons drawn from a closed vocabulary; anything not extracted (attachments, formatted bodies, envelope fields) appears as a reason or a count, never as silence. The vocabulary is closed when the schemas are frozen; until then new reasons may appear.
7. **Names are not data.** Directory names derive from the naming ADR and carry no meaning a reader may rely on; everything a tool needs is in the JSON files (see the [identity ADR](message-and-folder-identity-and-provenance.md)).
8. **Provenance is recorded, privacy is the user's.** Archives contain message content by design; what a tool prints about them stays content-free ([output posture ADR](export-output-posture-consent-and-ownership.md)).

**Evolution policy once a schema is frozen (numbered `schema_version`):**

* **Additive changes** (new optional fields, new reasons in an open list, new files) keep the version number's major part; readers must ignore fields they do not know.
* **Breaking changes** (removing or renaming a field, changing a meaning or type, reordering semantics) require a new major version, are recorded in the change history, and mean existing archives are re-exported rather than rewritten in place.
* A reader must refuse, with a clear message, a `schema_version` major it does not know; it must not guess.
* `0.1-draft` files carry no compatibility promise and are expected to be re-exported.

### Consequences

* Good, because byte-identical archives and a single tree hash are possible, and reviewers can diff two exports directly.
* Good, because future readers can detect the writer's format and refuse politely.
* Good, because the draft label is honest about what is and is not settled.
* Bad, because fixing field order and a no-maps rule constrains how the model can be serialized.
* Bad, because until M4g every archive written is a draft that will have to be re-exported.

### Confirmation

* Golden files (`tests/golden/hello/`) compare `message.md` and `metadata.json` byte for byte, both from the pure renderers and from a real export; CI runs them on Linux and Windows.
* Determinism is tested (two exports to different directories, byte-identical), and was observed on the real `.msg` corpus: the same `export_tree_sha256` across three runs and two output directories. Read-back compares every written file with what was planned.
* Rules 1, 2, 3, 4, and 5 hold for the M4c output by construction (the renderers are pure, use ordered structs, and are fed no paths or times other than the source's) and are checked by the golden and determinism tests. Rule 6 is partly implemented: M4c always writes `partial` with reasons, because most fields are not extracted yet.
* Planned: JSON schema tests on synthetic archives, a check that no absolute path or export-time value appears in any metadata file, and archive read-back verification (M4g, M4j).
* Not yet confirmed: that a numeric `schema_version` scheme and the additive/breaking split are enough; the closed reason vocabulary is not yet defined.

## Pros and Cons of the Options

### A. Binding file rules, versioning, additive evolution, schemas frozen when proven

* Good, because it fixes what is known and defers what is not.
* Bad, because early archives are explicitly disposable.

### B. Freeze the full schemas now

* Good, because readers could be written immediately.
* Bad, because the envelope, bodies, attachments, and identity fields do not exist yet; a frozen schema would be wrong or incomplete, and any later change is a breaking one.

### C. No versioning

* Good, because less to write.
* Bad, because old archives become unreadable without anyone noticing.

### D. YAML front matter in `message.md`

* Good, because one file per message.
* Bad, because the archive ADR chose separate `metadata.json` so Markdown stays a projection; front matter also mixes content and metadata and is hard to keep deterministic for large property bags.

## More Information

* Draft field lists as built: [`docs/plans/m4a-export-rules.md`](../docs/plans/m4a-export-rules.md) section 3.2, and a worked example in [`docs/examples/m4c-example-archive.md`](../docs/examples/m4c-example-archive.md).
* Revisit when: the metadata stage (M4g) defines the closed status vocabulary and freezes the first numbered version.
