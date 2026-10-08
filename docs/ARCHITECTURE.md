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
`--verify` (and, since M4d, `--verify-envelope`).

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
normalized domain model: the MSG adapter decodes the values such a model
would be built from. (Since M4c and M4d, the export path assembles some of
them into a small model; see the sections below.)

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

- `model.rs` began as the seed of the normalized model (M4b-4): subject, Internet message ID, time, the plain-text body, and which other body forms exist. It imports no adapter module.
- `archive.rs` has no I/O, so what is written is testable byte for byte (golden files are committed under `tests/golden/`).
- `export.rs` never overwrites or deletes what it did not generate: the target must be absent, empty, or a directory whose root `folder.json` was written by `tsp` for the same source; its own files are replaced only with consent (`--overwrite` or a yes at the prompt).
- Each file is replaced atomically (written under a short numeric name in `.tsp-tmp`, then renamed). The archive as a whole is not all-or-nothing: the root `folder.json` is first written as `incomplete` and replaced with the `complete` version last, so an interrupted run is recognizable.
- Not exported yet: attachments and embedded messages (counted and recorded as not extracted), HTML and RTF bodies, the remaining message properties, and `.pst` input.

## M4d boundary: the envelope

M4d adds who a message was from and to, and when, through the same path. The
extraction is a separate adapter module so that the pure model and the pure
renderers stay free of MSG details.

```text
.msg file
 |
 +-- oxmsg_envelope.rs   reads, from the message's properties and its recipient
 |                       storages: sender, sent-representing identity, To/Cc/Bcc
 |                       recipients (display name, address type, address, and
 |                       SMTP address kept separate), submit and delivery times,
 |                       importance, sensitivity, conversation topic and index,
 |                       transport headers; strings via the code page chain;
 |                       nothing derived or repaired
 |
 +-- source_msg.rs       `read_message_content` -> model.rs `Envelope`
 |                       (with `Address` and `Recipient`) inside `MessageContent`
 |
 +-- archive.rs          pure rendering: a header list in message.md (each
                         value in a code span; the SMTP address preferred and
                         an Exchange distinguished name never shown) and an
                         `envelope` object in metadata.json (stored ticks plus
                         one UTC string; stored integers for importance and
                         sensitivity)
```

- Property identifiers, how well each is confirmed, and the extraction rules are in [`plans/m4d-envelope-properties.md`](plans/m4d-envelope-properties.md).
- A recipient row that is not To, Cc, or Bcc (the originator row, an unknown type, an unreadable type) is counted in `recipients_unlisted` and not listed.
- `verify_envelope.rs` is the envelope's check: it runs the same extraction and `msg_parser` over the same files and prints match/mismatch counts for the subject, sender, and recipients. It does not touch the export path.
- The status reason `envelope_not_extracted` was replaced by `other_properties_not_preserved`: the envelope is now written, but the rest of the property bag still is not.

## M4e-1 boundary: de-encapsulation (built in v0.1.26)

M4e-1 adds the first body-conversion step, kept pure and outside the writer for now.

```text
PidTagRtfCompressed (bytes)
 |
 +-- compressed-rtf          MS-OXRTFCP decompression (existing)
 |
 +-- rtf_deencap.rs          pure: recognize (10-token rule) -> tokenize -> interpret
 |                           (htmltag groups, htmlrtf suppression, font code pages,
 |                            \uN with fallbacks, skipped destinations) -> UTF-8 HTML
 |                           plus content-free diagnostics; bounded depth and output
 |
 +-- verify_deencap.rs       --verify-deencap: the recovered HTML against msg_parser's
                             html_from_rtf(), graded; recognition rule against the
                             whole-document \fromhtml1 search
```

- `rtf_deencap` imports only the `PT_STRING8` decoders from `oxmsg_decode`; it reads no file and knows no adapter type. Nothing in the export calls it yet; M4e-2 (the body pipeline) will.
- The recovered text is UTF-8, so a `charset=` declaration inside it is stale; the diagnostics flag one.

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

`tsp --verify-envelope` does the same for the envelope: the custom path's
subject, sender, and recipients against `msg_parser`'s, field by field,
reporting for each email which of the custom path's two properties
(`PidTagEmailAddress` or the SMTP address) equalled the oracle's string. The
fields `msg_parser` does not expose (sent-representing, times, importance,
sensitivity, conversation fields, transport headers) are reported only as
presence counts, so they have no independent check; they rest on the
specification and the unit tests. Like `--verify`, it scans a directory
non-recursively.

The PST side has no independent-oracle comparison yet.

The planner is verified differently: `verify_plan` is a gate over every plan
(its violation count prints under `--dry-run` and gates `export`), unit and
property tests cover the naming and planning rules, and `verify-split.ps1`
checks that a change leaves the existing outputs byte-identical to an earlier
tag. The writer is verified by synthetic tests (golden files, determinism,
the overwrite matrix) and was run on real corpus messages at v0.1.23.1; the
envelope output was examined on a couple of exported messages at v0.1.25.1,
and a full corpus export for that tag has not been reported.

## Code layout

The code lives in `src/`, one module per seam:

- `cli` — arguments and input classification; `main` — dispatch.
- `shared` — vocabulary, the encapsulated-HTML check, shared counters.
- `pst` — the PST diagnostic; `msg_report` — the shared MSG report.
- `oxmsg_classify`, `oxmsg_decode`, `oxmsg_structure`, `oxmsg_extract` — the custom MS-OXMSG parser, from naming conventions through extraction; `oxmsg_envelope` — the envelope extraction (M4d).
- `verify` — the `--verify` comparison and structural gates; `verify_envelope` — the `--verify-envelope` comparison (M4d); `verify_deencap` — the `--verify-deencap` comparison (M4e-1).
- `rtf_deencap` — the pure MS-OXRTFEX de-encapsulation (M4e-1).
- `naming`, `plan` — the pure naming rules and export planner (M4b).
- `source_msg`, `source_pst`, `dry_run` — readers that build the planner's source tree, and the `--dry-run` report (M4b); `source_msg` also re-reads message content, including the envelope, for export.
- `model`, `archive`, `export` — the model so far (message content and envelope), the pure renderers, and the writer with its preflight and consent rules (M4c, extended by M4d).
- `tests` — most of the older unit tests; the newer modules keep theirs beside their code.

The README lists the contents of each module.

Dependency direction today: `shared` depends on nothing in the crate; the PST diagnostic and the `oxmsg_*` modules depend on `shared`; `verify` depends on the extraction and structure modules; `naming` and `plan` depend on no adapter module; `source_msg` and `source_pst` depend on their adapters and on `plan`; `model` depends on nothing; `archive` depends on `model` and `naming`; `export` depends on `archive`, `model`, `plan`, `naming`, `dry_run`, and `source_msg`. The envelope modules follow the same pattern: `oxmsg_envelope` is an adapter module that depends on `oxmsg_classify`, `oxmsg_decode`, `oxmsg_extract`, and `shared` and produces `model` types (it also takes the submit and delivery time property IDs from `dry_run`, where they were first defined, which is a small layering oddity: an adapter importing from the reporting module), and `verify_envelope` depends on `model`, `source_msg`, `dry_run` (for the subject-marker stripping), and `verify` (for the shared comparison tallies). Every item is `pub(crate)`, so the module boundaries are organizational, not yet a designed API. `naming.rs` and `plan.rs` still carry `#![allow(dead_code)]`; the plan was to drop it when the writer landed, and it has not been tried yet. The full normalized model (M4b-4) has not been written: bodies with raw bytes, attachments, the raw property bag, named properties, and diagnostics are not modelled yet.
