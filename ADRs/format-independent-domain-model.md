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

**Not yet confirmed.** The normalized `OutlookItem` model does not exist in
the code yet; both adapters currently produce privacy-safe diagnostic
counters, not normalized items.

What the evidence does show:

- The PST diagnostic (M1) is written against the `outlook-pst` crate's
  `Message`/`Folder` abstractions rather than raw PST byte structures, and
  the MSG path (M2–M3) is written against its own decoded-property layer
  rather than the container bytes. Both are consistent with this decision.
- The two adapters converge on one vocabulary at the diagnostic level.
  They share the same counter types (`BodyCounters`, `CountStats`,
  `ZeroByteStats`), the same MS-OXRTFEX `\fromhtml1` detection function
  (`check_compressed_rtf_bytes`), and the same report keys, so they cannot
  drift apart silently. The default `.msg` report has the same shape the
  PST report uses.
- The MSG adapter's decoded values (typed fixed values, decoded strings,
  resolved named properties) are the raw material a normalized model would
  be built from.

That is convergence in the diagnostics, not proof of the decision. The real
test is unchanged: whether the MSG adapter can feed the *same* normalized
model as the PST adapter, without changes to the M4 Markdown/archive
engine. That confirmation is still outstanding, and it is now unblocked on
the MSG side, since the custom parser is the production path (see
[custom-MS-OXMSG-parser-grad-to-prod-msg-path.md](custom-MS-OXMSG-parser-grad-to-prod-msg-path.md)).

## More Information

See [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) for the overall
pipeline this model sits in, and
[docs/verification/format-coverage.md](../docs/verification/format-coverage.md)
for what each format currently supports against this model.
