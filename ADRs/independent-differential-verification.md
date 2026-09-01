---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Independent differential verification

## Context and Problem Statement

Teaspoon's own tests can confirm that its output matches what teaspoon's
own code produces, but that doesn't confirm the output is *correct* — a
systematic misreading of the PST/MSG spec would pass teaspoon's own tests
just as easily as a correct reading would. How should teaspoon's extraction
be checked against something other than itself?

## Decision Drivers

- Self-consistency (teaspoon agreeing with teaspoon) is not evidence of
  correctness against the specification.
- Mature independent PST/MSG implementations already exist and have their
  own track record against real-world files.
- This needs to stay independent of the normative-authority decision (see
  [microsoft-specifications-as-normative-authority.md](microsoft-specifications-as-normative-authority.md)):
  the oracle used to check output must not also be the definition of what
  "correct" means, or the check becomes circular.

## Considered Options

- Independent differential verification: compare teaspoon's extraction
  against independent implementations (e.g. libpff, libpst for PST; the
  Rust `msg_parser` crate as an initial MSG comparison candidate) on the
  same input files.
- Rely solely on teaspoon's own unit and behavioral tests.
- Manual spot-checking of output against Outlook's own rendering, without
  automated cross-implementation comparison.

## Decision Outcome

Chosen option: "Independent differential verification", because it is the
only option that checks teaspoon's extraction against something other than
itself; unit/behavioral tests alone (option two) can only catch regressions
against teaspoon's own prior behavior, and manual spot-checking (option
three) doesn't scale and isn't repeatable. Agreement with the same
implementation that produced the result under test is explicitly not
considered independent verification.

### Consequences

- Good, because it catches systematic misreadings of the specification that
  self-consistent tests cannot.
- Good, because it produces reusable, repeatable evidence rather than
  one-off manual checks.
- Bad, because it requires integrating and maintaining comparisons against
  external tools/libraries (libpff, libpst, `msg_parser`) that `teaspoon` does
  not otherwise depend on.
- Bad, because independent implementations can themselves have bugs or
  spec deviations, so disagreement doesn't automatically mean `teaspoon` is
  wrong — each discrepancy needs to be triaged against the normative spec,
  not resolved by majority vote.

### Confirmation

**This ADR records that the decision to do differential verification has
been accepted — it does not record that differential verification has been
performed.** As of v0.1.2, no comparison against libpff, libpst, or
`msg_parser` has been run. This is tracked as outstanding work; see
[docs/verification/test-strategy.md](../docs/verification/test-strategy.md)
for where it sits in the overall evidence ladder.

## More Information

Candidate PST oracles: libpff, libpst. Candidate MSG oracle: the Rust
`msg_parser` crate. None have been integrated yet.
