---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Format-independent Outlook domain model

## Context and Problem Statement

PST and MSG are different physical container formats, but both ultimately
serialize the same Outlook/MAPI concepts: Message, Recipient, Attachment,
and Property. `teaspoon` needs to mine both formats (PST now, MSG in a later
milestone) into the same kind of durable Markdown+metadata archive. Should
the Markdown renderer and archive writer be coupled to the PST or MSG
parser they happened to be fed by, or should ingestion be decoupled from
rendering behind a shared, format-independent model?

## Decision Drivers

- Avoid duplicating rendering/archive logic once MSG ingestion (M2) begins.
- Keep format-specific parsing quirks isolated from the rest of the
  pipeline, so a defect or limitation in one format's adapter can't leak
  into the other's output.
- Preserve raw/unknown properties even when the normalized model doesn't
  have a named field for them (see
  [loss-aware-normalized-representation.md](loss-aware-normalized-representation.md)).

## Considered Options

- A shared, format-independent `OutlookItem` model that both a PST adapter
  and a future MSG adapter normalize into before any rendering occurs.
- Separate, format-specific rendering paths (a "PST renderer" and an "MSG
  renderer") that each go straight from parser output to Markdown.
- Defer the question by building PST-only for now and deciding later,
  once MSG ingestion actually starts.

## Decision Outcome

Chosen option: "A shared, format-independent `OutlookItem` model", because
it is the only option that lets the same Markdown/archive pipeline serve
both formats without duplicating rendering and archive-layout logic, and
because deferring the decision (option three) would risk baking PST-specific
assumptions into the renderer by accident during M1, making the eventual
MSG adapter (M2) more disruptive to add than if the boundary is drawn now.

### Consequences

- Good, because the same Markdown/archive pipeline handles both formats.
- Good, because format-specific parsing remains isolated — a PST parsing
  limitation can't silently change MSG output behavior, or vice versa.
- Bad, because the model must retain raw/unknown properties where feasible,
  which adds complexity the model would not need if it only had to satisfy
  one format.
- Bad, because some format-specific provenance (e.g., which container the
  item came from, and any format-specific extraction caveats) still needs
  to be carried through the normalized model rather than discarded.

### Confirmation

M1's PST spike (`tsp`) already writes its diagnostics against a
`Message`/`Folder` abstraction from the `outlook-pst` crate rather than
raw PST byte structures, which is consistent with this decision but does
not yet prove it: the real test is whether the M2 MSG adapter can be
written to feed the *same* normalized model without changes to the M4
Markdown/archive engine. That confirmation is still outstanding.

## More Information

See [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) for the overall
pipeline this model sits in, and
[docs/verification/format-coverage.md](../docs/verification/format-coverage.md)
for what each format currently supports against this model.
