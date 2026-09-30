---
status: "accepted"
date: 2026-09-29
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Custom MS-OXMSG parser graduates to the production MSG path

Status history: proposed 2026-09-23; accepted 2026-09-29, when M3f shipped
the default-path flip with its evidence gate satisfied (see Confirmation).
The transitional flags were retired in M3g the same day.

## Context and Problem Statement

As of v0.1.9.9, `.msg` input was handled by `msg_parser` by default, with
an experimental `cfb`-based custom parser available via `--oxmsg` (a
flag since retired). The
custom path had by then verified, against the full 29-file fixture corpus,
complete structural enumeration (M2.x: every CFB entry accounted for and
classified), property-entry decoding (type/ID/flags/variable-length size-
reserved, cross-checked against the attachment Reserved-field sentinel),
and named-property resolution (GUID set and numeric/string identity,
confirmed byte-for-byte against real fixture data after a bit-layout
defect was found and fixed). It still decoded no property *value*.
`msg_parser` remained the only path that produced teaspoon's MSG
diagnostic output (message class, body-type detection, recipient/
attachment classification), and had the only public API teaspoon used for
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
  `msg_parser`; retain `msg_parser` only as a differential-verification
  oracle, never a runtime fallback.
- Graduate the custom parser as the default, and add an explicit,
  user-facing `--msg-parser` flag to fall back to the legacy path
  indefinitely.
- Keep `msg_parser` as the default indefinitely; use the custom path only
  for the specific gap it closes (raw property iteration), composed
  alongside `msg_parser` rather than replacing it.

## Decision Outcome

Chosen option: "Graduate once verified, `msg_parser` becomes an oracle
only, no runtime fallback flag," because it is the only option that
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
- Bad, because it required M3a-M3e's real value-extraction and parity work
  before the default could flip. The transitional `--oxmsg` and `--extract`
  flags that bridged the flip were removed in M3g.
- Bad, because relying on `msg_parser` only as an oracle forgoes a second,
  independently-maintained implementation's ongoing bug fixes as runtime
  behavior. The oracle role keeps their value as verification signal.

### Confirmation

M3e's differential-verification run against the 29-file corpus, documented
in [docs/verification/m3-results.md](../docs/verification/m3-results.md),
was the fitness function: the default flips (M3f) only once that document
shows parity or better, field by field, with every disagreement triaged
against the Microsoft specifications rather than resolved by which tool
said what.

That condition was met. The corpus showed parity on every comparable field
and exactly two differences, both resolved in the custom path's favor
against the specifications (the MS-OXRTFCP dictionary, and embedded-message
opening). M3f then flipped the `.msg` default to the custom path. A
default-flag run over the 29 fixtures produced output identical to the
earlier `--extract` output, the build was clean, and all tests passed
(56 at the flip).

How the outcome differs from the decision as written:

- **`msg_parser` is a normal Cargo dependency, not a dev-dependency.** The
  option text says "dev/test"; the `--verify` harness ships in the `tsp`
  binary, and a dev-dependency is not available to the binary. The
  oracle-only role is enforced by where the crate is called (only from
  `--verify`), and no default code path calls it. The Cargo section itself
  does not enforce this. Moving `--verify` behind a Cargo feature or into a
  separate test-only binary would make `msg_parser` a true dev-dependency;
  that is an open option, not a scheduled change.
- **The transitional flags are gone.** `--extract` had become a redundant
  alias of the default, and `--oxmsg` a structural diagnostic whose gate
  counters were folded into `--verify`. M3g removed both, and passing
  either is now a command-line error. Neither ever fell back to
  `msg_parser`, so the "no runtime fallback flag" clause held throughout.
  The structural breakdown `--oxmsg` printed is now printed by `--verify`,
  and only when a structural gate is nonzero.

Follow-up hardening, recorded in the M3 results file: the both-paths
comparison is re-runnable as a corpus-gated test
(`fixture_corpus_verify_is_clean`, enabled by `TSP_FIXTURE_DIR`), so that
the M3e evidence can be re-checked on demand instead of being a one-time
record. It cannot run in CI, because the fixtures are not in the repository,
and it has not been recorded as run against the corpus. What was run is
`tsp --verify` over the 29 fixtures, which reported parity on every
comparable field and zero structural gate violations, on a build with 64
passing tests.

## More Information

Supersedes no prior ADR; executes the outstanding work recorded in
"independent differential verification" for the MSG side specifically.
Revisit if a future comparison surfaces a disagreement that can't be
resolved against the spec text alone (e.g. a real-world `.msg` producer
doing something neither document anticipates) — that would be grounds to
reconsider scope, not to abandon the verification step. The corpus has no
`PT_STRING8` property, so ANSI-encoded `.msg` input is the largest
remaining area where the graduated path has not met real data.
