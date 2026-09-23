---
status: "proposed"
date: 2026-09-23
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Custom MS-OXMSG parser graduates to the production MSG path

## Context and Problem Statement

As of v0.1.9.9, `.msg` input is handled by `msg_parser` by default, with
an experimental `cfb`-based custom parser available via `--oxmsg`. The
custom path has since verified, against the full 29-file fixture corpus,
complete structural enumeration (M2.x: every CFB entry accounted for and
classified), property-entry decoding (type/ID/flags/variable-length size-
reserved, cross-checked against the attachment Reserved-field sentinel),
and named-property resolution (GUID set and numeric/string identity,
confirmed byte-for-byte against real fixture data after a bit-layout
defect was found and fixed). It still decodes no property *value*.
`msg_parser` remains the only path that produces teaspoon's current MSG
diagnostic output (message class, body-type detection, recipient/
attachment classification), and has the only public API teaspoon uses for
it. Should the custom parser become the production path, and if so, what
role does `msg_parser` retain?

## Decision Drivers

- Design principle 4 (unknown/unmapped properties preserved or reported,
  not silently discarded) is unreachable through `msg_parser` alone — the
  motivating gap for building the custom path in the first place.
- ADR "independent differential verification" already designates
  `msg_parser` as the MSG oracle and records that comparison as
  outstanding work, not yet performed.
- `msg_parser`'s `html_from_rtf()` was previously found unreliable and
  replaced with a spec-correct check (M1) — a precedent that trusting a
  secondary implementation without checking it against the spec has
  already cost real correctness once on this project.
- The custom path has no production track record beyond one 29-file
  corpus; `msg_parser` has its own, independent one.

## Considered Options

- Graduate the custom parser to be the default MSG path once value
  extraction (M3a-M3e below) is built and differentially verified against
  `msg_parser`; retain `msg_parser` only as a dev/test differential-
  verification dependency, never a runtime fallback.
- Graduate the custom parser as the default, and add an explicit,
  user-facing `--msg-parser` flag to fall back to the legacy path
  indefinitely.
- Keep `msg_parser` as the default indefinitely; use the custom path only
  for the specific gap it closes (raw property iteration), composed
  alongside `msg_parser` rather than replacing it.

## Decision Outcome

Chosen option: "Graduate once verified, `msg_parser` becomes a dev/test
oracle only, no runtime fallback flag," because it is the only option that
keeps `msg_parser`'s role consistent with what the differential-
verification ADR already decided, while still resolving the raw-property-
iteration gap that motivated the custom parser. A permanent fallback flag
would let the two implementations diverge silently in production exactly
the way `html_from_rtf()` already did once, and would blur the oracle/
subject distinction the verification ADR depends on staying separate.

### Consequences

- Good, because it resolves design principle 4's gap without abandoning
  the independent-verification safety net the project already committed
  to for MSG.
- Good, because `msg_parser`'s disagreements stay signals to investigate
  rather than being silently absorbed by users routing around a rough
  edge with a fallback flag.
- Bad, because it requires M3a-M3e's real value-extraction and parity work
  before the flag can be removed — this is not a quick win, and
  `--oxmsg`/`msg_parser` both stay in the tree until it's done.
- Bad, because dropping `msg_parser` as a runtime dependency later removes
  a second, independently-maintained implementation's ongoing bug fixes as
  free verification signal — mitigated by keeping it as a dev-dependency
  rather than removing it entirely.

### Confirmation

M3e's differential-verification run against the 29-file corpus, documented
in `docs/verification/` the way `m1-results.md`/`m2-results.md` are, is
the fitness function: the default flips (M3f) only once that document
shows parity or better, field by field, with every disagreement triaged
against MS-OXMSG/MS-OXCMSG rather than resolved by which tool said what.

## More Information

Supersedes no prior ADR; executes the outstanding work recorded in
"independent differential verification" for the MSG side specifically.
Revisit if M3e surfaces a disagreement that can't be resolved against the
spec text alone (e.g. a real-world `.msg` producer doing something neither
document anticipates) — that would be grounds to reconsider scope, not to
abandon the verification step.
