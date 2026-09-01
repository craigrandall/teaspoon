---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Loss-aware normalized representation

## Context and Problem Statement

Not every property, attachment, or body variant in a real PST/MSG item will
be extractable — properties can be missing, malformed, externally
referenced (`ATTACH_BY_REFERENCE`), or simply not yet implemented by
teaspoon. When extraction of some piece of an item fails or is incomplete,
should that be reported explicitly, or is it acceptable for the normalized
representation to just omit what it couldn't get, the way many best-effort
exporters do?

## Decision Drivers

- `teaspoon` is meant to be a durable archive, not a disposable export — a
  silent gap discovered years later, after the source PST is gone, cannot
  be recovered.
- Users need to be able to distinguish "this item truly had no attachment"
  from "this item had an attachment `teaspoon` couldn't extract."
- This is one of the four foundational M0 constraints, so it needs to hold
  consistently across every future adapter (PST, MSG, and beyond), not be
  decided per-feature.

## Considered Options

- Loss-aware: the normalized representation retains raw/unknown properties
  where practical, and records explicit per-item extraction diagnostics
  distinguishing complete / partial / failed.
- Best-effort silent: extract what's straightforward and simply omit
  anything that fails or isn't yet supported, with no diagnostic record.
- Fail-closed: abort processing of an entire message if any property or
  attachment can't be fully extracted.

## Decision Outcome

Chosen option: "Loss-aware", because it is the only option consistent with
teaspoon's purpose as a durable archive rather than a best-effort exporter:
a best-effort-silent approach makes gaps indistinguishable from genuine
absence, and a fail-closed approach would discard everything `teaspoon` *did*
successfully extract from a message just because one property or attachment
failed.

### Consequences

- Good, because gaps are visible and auditable rather than silently
  indistinguishable from "there was nothing there."
- Good, because a message can still be archived with most of its content
  even when one property or attachment can't be fully extracted.
- Bad, because every extraction path must be written to report its own
  completeness, rather than just returning whatever it managed to get.
- Bad, because the extraction-status vocabulary (complete / partial /
  failed, and the specific reasons within each) must be designed and kept
  consistent across every future adapter, which is more upfront design work
  than an ad hoc "best effort" approach.

### Confirmation

M1's P4a diagnostics already follow this: unreadable `PidTagMessageClass`
values are counted as `message_class_read_errors` rather than silently
dropped, and P2's `message_open_errors`/`folder_open_errors` counters follow
the same pattern for structural failures. See
[docs/verification/m1-results.md](../docs/verification/m1-results.md).
Attachment content and per-recipient classification (in progress as of
P4b) will need to extend this same status vocabulary once implemented.

## More Information

Unsupported or externally referenced attachment content (e.g.
`ATTACH_BY_REFERENCE`) must never be silently reported as preserved; it
must be flagged as partial, per this decision. See
[deterministic-markdown-archive.md](deterministic-markdown-archive.md) for
where these diagnostics are expected to surface in the eventual archive
(`metadata.json`).
