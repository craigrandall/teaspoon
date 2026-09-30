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

## Code layout

All code is in `src/main.rs`, ordered top to bottom: CLI and input
classification; shared detection and vocabulary; the PST diagnostic; the MSG
report; the custom MS-OXMSG parser (classification, decoding, structural
walk, extraction); differential verification; tests. The README lists the
current functions in each section. The file has not been split into modules.
