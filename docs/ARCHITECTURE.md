# Architecture

## Intent

`teaspoon` is a format-independent Outlook item mining tool.

The key boundary is:

```text
PST / MSG serialization
        |
        v
format adapter
        |
        v
normalized Outlook item
        |
        +--> Markdown
        +--> metadata
        +--> attachment archive
        +--> diagnostics
```

PST and MSG must not leak into the renderer or archive writer.

`architecture.mmd` and `pipeline.mmd` in this folder draw this target
architecture. They describe where the project is going, not what the code
does today; the sections below say what exists.

## Domain boundary

The eventual normalized model should represent:

- source/provenance
- folder location where applicable
- message class
- message properties
- recipients
- plain/HTML/RTF bodies
- attachments
- embedded messages
- named properties
- extraction diagnostics

The raw property bag is intentional: Outlook's property model is larger than the subset required by Markdown.

## M1 boundary

M1 intentionally stops at:

```text
PST
 |
 +-- open store
 |
 +-- IPM subtree
 |     |
 |     +-- folders
 |           |
 |           +-- hierarchy
 |           +-- contents
 |                 |
 |                 +-- message entry IDs
 |                 +-- message properties
 |
 +-- deterministic inventory
```

M1 does not create the final normalized domain model. That prevents a spike implementation from prematurely becoming the architecture.

## M2–M3 boundary: the MSG adapter

The MSG side started in M2 as a diagnostic built on the `msg_parser` crate,
and became a custom adapter in M3. Since M3f the custom adapter is the
production `.msg` path; `msg_parser` is used only as the oracle behind
`--verify`.

```text
.msg file
 |
 +-- MS-CFB container            (generic `cfb` crate)
 |
 +-- MS-OXMSG naming layer       (teaspoon)
 |     |
 |     +-- entry classification  (every entry accounted for, or counted)
 |     +-- properties streams    (typed fixed values, entry shapes)
 |     +-- value streams         (UTF-16LE, PT_STRING8 via a code page
 |     |                          chain, binary, CLSID)
 |     +-- named-property map    (GUID / numeric / string names)
 |     +-- recipient, attachment, and embedded-message storages
 |
 +-- extraction layer            (message class, bodies, recipients,
 |                                attachments, embedded message class)
 |
 +-- deterministic inventory     (same report shape as the PST side)
```

The extraction layer reads real content into memory to compute booleans and
counts, and prints none of it. Like M1, this boundary stops before the
normalized domain model. The MSG adapter now decodes the values such a model
would be built from, but nothing yet assembles them into a normalized item.

## M4b boundary: planning an export without writing one

M4b adds the part of the export that needs no I/O: deciding every name and
path before anything is written.

```text
.pst / .msg input
 |
 +-- source reader               (source_pst.rs / source_msg.rs)
 |     reads names, times, attachment names, embedded messages into a
 |     source tree, plus a content-free census of what it saw
 |
 +-- naming rules                (naming.rs: sanitize, normalize, truncate to
 |                                a UTF-16 unit budget, reserved names,
 |                                collision key)
 |
 +-- export planner              (plan.rs: pure; source tree + Policy -> planned
 |     entries with final names, flags, counters)
 |
 +-- plan verifier               (plan.rs `verify_plan`: collisions, over-budget
 |                                paths, invalid names, unit mismatches)
 |
 +-- --dry-run report            (dry_run.rs: counts only)
```

The planner and the naming rules depend on no adapter module; the source
readers are the only code that knows about PST or MSG here. Nothing is
written by any code in this boundary.

## M4c boundary: the first writer (`.msg` input, plain-text body)

M4c consumes the same plan and writes an archive. It is a walking skeleton:
one thin path from a `.msg` input to files on disk, with the pieces that will
grow later already separated.

```text
.msg file / directory
 |
 +-- source_msg.rs   source tree + map: message source ID -> its .msg file
 |
 +-- plan.rs         the same plan `--dry-run` reports; export refuses a plan
 |                   that breaks a gate (exit code 2, nothing written)
 |
 +-- source_msg.rs   `read_message_content`: re-reads one message when the
 |                   writer needs it -> model.rs `MessageContent`
 |
 +-- archive.rs      pure rendering, no I/O: message.md, metadata.json,
 |                   folder.json (draft schema "0.1-draft")
 |
 +-- export.rs       render everything in memory -> preflight against the
                     target (counts only) -> consent -> write through
                     <out>/.tsp-tmp -> read back -> content-free report
```

- `model.rs` is only the seed of the normalized model (M4b-4): subject, Internet message ID, time, the plain-text body, and which other body forms exist. It imports no adapter module.
- `archive.rs` has no I/O, so what is written is testable byte for byte (two golden files are committed under `tests/golden/`).
- `export.rs` never overwrites or deletes what it did not generate: the target must be absent, empty, or a directory whose root `folder.json` was written by `tsp` for the same source; its own files are replaced only with consent (`--overwrite` or a yes at the prompt).
- Each file is replaced atomically (written under a short numeric name in `.tsp-tmp`, then renamed). The archive as a whole is not all-or-nothing: the root `folder.json` is first written as `incomplete` and replaced with the `complete` version last, so an interrupted run is recognizable.
- Not exported yet: attachments and embedded messages (counted and recorded as not extracted), sender/recipients/headers, HTML and RTF bodies, and `.pst` input.

## Verification structure

`tsp --verify` runs the custom MSG path and `msg_parser` over the same files
and prints match and mismatch counts per field, never the values compared.
It also runs the custom path's structural accounting and prints its gate
counters, which must all read 0, with a single `structural_gate_violations`
total; when any gate is nonzero it also prints the structural breakdown for
triage. Disagreements are triaged against the Microsoft specifications, not
settled by majority vote (see
[ADR: Microsoft specifications as normative authority](../ADRs/microsoft-specifications-as-normative-authority.md)
and [ADR: independent differential verification](../ADRs/independent-differential-verification.md)).

The PST side has no independent-oracle comparison yet.

The planner is verified differently: `verify_plan` is a gate over every plan
(its violation count prints under `--dry-run` and gates `export`), unit and
property tests cover the naming and planning rules, and `verify-split.ps1`
checks that a change leaves the existing outputs byte-identical to an earlier
tag. The writer is verified by synthetic tests (golden files, determinism,
the overwrite matrix); a run on real corpus messages is still to be done.

## Code layout

The code lives in `src/`, one module per seam:

- `cli` — arguments and input classification; `main` — dispatch.
- `shared` — vocabulary, the encapsulated-HTML check, shared counters.
- `pst` — the PST diagnostic; `msg_report` — the shared MSG report.
- `oxmsg_classify`, `oxmsg_decode`, `oxmsg_structure`, `oxmsg_extract` — the custom MS-OXMSG parser, from naming conventions through extraction.
- `verify` — the `--verify` comparison and structural gates.
- `naming`, `plan` — the pure naming rules and export planner (M4b).
- `source_msg`, `source_pst`, `dry_run` — readers that build the planner's source tree, and the `--dry-run` report (M4b); `source_msg` also re-reads message content for export.
- `model`, `archive`, `export` — the seed model, the pure renderers, and the writer with its preflight and consent rules (M4c).
- `tests` — most of the older unit tests; the newer modules keep theirs beside their code.

The README lists the contents of each module.

Dependency direction today: `shared` depends on nothing in the crate; the PST diagnostic and the `oxmsg_*` modules depend on `shared`; `verify` depends on the extraction and structure modules; `naming` and `plan` depend on no adapter module; `source_msg` and `source_pst` depend on their adapters and on `plan`; `model` depends on nothing; `archive` depends on `model` and `naming`; `export` depends on `archive`, `model`, `plan`, `naming`, `dry_run`, and `source_msg`. Every item is `pub(crate)`, so the module boundaries are organizational, not yet a designed API. `naming.rs` and `plan.rs` still carry `#![allow(dead_code)]`; the plan was to drop it when the writer landed, and it has not been tried yet. The full normalized model (M4b-4) has not been written.
