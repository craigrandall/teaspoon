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

The diagnostics follow this decision at the counter level on both adapters.
What does not exist yet is the per-item complete / partial / failed status
vocabulary in a normalized model: today the loss accounting is aggregate
counters in the privacy-safe inventory, not a per-item record.

PST side (M1): unreadable `PidTagMessageClass` values are counted as
`message_class_read_errors` rather than silently dropped, and P2's
`message_open_errors`/`folder_open_errors` counters follow the same
pattern for structural failures. See
[docs/verification/m1-results.md](../docs/verification/m1-results.md).

MSG side (M2–M3): the custom MS-OXMSG path extends the same pattern.

- Every CFB entry is classified, and anything else is counted rather than
  dropped; `entry_accounting_gap_total` and `unrecognized_entries_total`
  both read 0 on the 29-file corpus. Content the tool deliberately does not
  interpret (a Word document's own OLE streams inside a custom attachment
  storage) is counted as `opaque_payload`, not omitted.
- A by-value attachment whose data stream cannot be read is reported as
  `attachments_data_stream_missing`, not counted as an empty file. This
  closed a real loss-accounting gap: the earlier MSG path could not tell an
  unreadable attachment from a zero-byte one.
- Property values that fail their structural checks each have a counter
  (`variable_value_stream_missing_total`, `variable_value_size_mismatch_total`,
  `variable_unicode_decode_errors_total`, and the fixed-value checks), all
  0 on the corpus.
- `PT_STRING8` values under a code page the decoder does not implement are
  reported (`variable_string8_unsupported_codepage_total`) and produce no
  text, rather than being decoded as if they were Windows-1252. This is
  covered by synthetic fixtures only, since the corpus has no `PT_STRING8`
  property.

These gates are reported by `tsp --verify` as a set of counters that must
all read 0, with a single `structural_gate_violations` total.

Attachment content extraction and the per-item status vocabulary will need
to extend this same pattern once implemented.

## More Information

Unsupported or externally referenced attachment content (e.g.
`ATTACH_BY_REFERENCE`) must never be silently reported as preserved; it
must be flagged as partial, per this decision. See
[deterministic-markdown-archive.md](deterministic-markdown-archive.md) for
where these diagnostics are expected to surface in the eventual archive
(`metadata.json`).
