---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Microsoft specifications as normative format authority

## Context and Problem Statement

PST and MSG behavior can be learned from at least three kinds of sources:
Microsoft's own Open Specifications documents (MS-PST, MS-OXMSG, MS-OXCMSG,
MS-OXOMSG, MS-OXPROPS), independent open-source implementations (libpff,
libpst, msg_parser, etc.), and empirical observation of real files. When
these sources disagree — or when a dependency's behavior is ambiguous —
which one governs how `teaspoon` interprets a property or structure?

## Decision Drivers

- Need a single, unambiguous tie-breaker when sources disagree, rather than
  resolving each conflict ad hoc.
- Independent implementations are valuable for *testing* teaspoon's output
  (see [independent-differential-verification.md](independent-differential-verification.md)),
  but using the same source as both the normative definition and the
  verification oracle would make that verification circular.
- The specifications are public, versioned, and stable, which real-world
  file samples and third-party source code are not.

## Considered Options

- Treat Microsoft's Open Specifications (MS-PST, MS-OXMSG, MS-OXCMSG,
  MS-OXOMSG, MS-OXPROPS) as normative, with independent implementations
  used only as evidence and validation oracles.
- Treat a mature independent implementation (e.g., libpff or libpst) as the
  de facto normative authority, since it reflects years of real-world
  compatibility fixes.
- Treat empirical behavior of sample files as authoritative, and write
  documentation as reverse-engineered notes rather than deferring to any
  external authority.

## Decision Outcome

Chosen option: "Treat Microsoft's Open Specifications as normative", because
they are the only source that is public, versioned, and authored by the
format's own originator, which makes disagreements resolvable by citation
rather than by whichever implementation `teaspoon` happened to consult first.
This also keeps differential verification against independent
implementations (ADR: independent-differential-verification) meaningful,
since the oracle and the specification being verified against are kept
separate.

### Consequences

- Good, because disagreements between dependencies or samples have a fixed,
  citable tie-breaker.
- Good, because it keeps normative authority and verification oracle
  distinct, so passing a differential test against libpff/libpst is
  actual evidence rather than agreement with itself.
- Bad, because the specifications are large and dense; correctly
  interpreting them takes more up-front effort than trusting an existing
  implementation's behavior.
- Bad, because real-world PST/MSG files sometimes deviate from the
  specification (producer bugs, legacy quirks), so normative-spec behavior
  and observed-file behavior can still diverge and must both be tracked.

## More Information

Primary specifications in scope: MS-PST, MS-OXMSG, MS-OXCMSG, MS-OXOMSG,
MS-OXPROPS. See [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) for where
these are cited in the current implementation.
