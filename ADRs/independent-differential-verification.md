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

**This ADR records the decision to do differential verification, and the
evidence that it has been performed for the MSG side only.** The PST side
is still outstanding.

MSG side (performed): `tsp --verify` runs the custom MS-OXMSG extraction
path and `msg_parser` over the same files and prints match/mismatch counts
per field, never the values compared. Over the 29-file `.msg` corpus it
showed parity on every comparable field and exactly two differences, both
triaged against Microsoft's specifications rather than settled by majority
vote or by trusting either implementation:

1. `msg_parser`'s preset LZFu dictionary diverges from the dictionary
   published in MS-OXRTFCP; the custom path (through `compressed-rtf`)
   matches the spec. The disagreement is −1 decompressed byte on one file
   of 29.
2. `msg_parser`'s `Attachment::as_message()` returns `None` on the corpus's
   one embedded-message attachment; the custom path opens it and reads its
   message class.

Both are recorded in [docs/verification/m3-results.md](../docs/verification/m3-results.md).
An independent-oracle result that flags a real defect in the oracle itself
(the dictionary) is the case the "each discrepancy needs triage" consequence
above anticipated.

Two structural limits on this evidence should be stated plainly:

- The corpus is a single 29-file set, and `msg_parser` is one oracle.
  `libpff` and `libpst` have not been integrated for either format.
- A comparison run is not a regression gate by itself. A corpus-gated test
  (`fixture_corpus_verify_is_clean`, enabled by setting `TSP_FIXTURE_DIR`)
  re-runs the comparison and the structural gates on demand, because the
  fixtures are deliberately not committed. CI cannot run it; it has been run
  by hand against the corpus and passed. The comparison itself has been
  re-run more than once (`tsp --verify` over the 29 fixtures: parity on
  every comparable field, the same two triaged differences, zero structural
  gate violations, identical output across runs), so the evidence has been
  reproduced. It is not a continuously enforced property, because nothing
  runs it automatically.

PST side (outstanding): no comparison against `libpff` or `libpst` has been
run. See [docs/verification/test-strategy.md](../docs/verification/test-strategy.md)
for where it sits in the overall evidence ladder.

## More Information

Candidate PST oracles: libpff, libpst (not yet integrated). MSG oracle in
use: the Rust `msg_parser` crate, through `--verify` only. It is a normal
Cargo dependency rather than a dev-dependency, because the verification
harness ships in the `tsp` binary; its oracle-only role is enforced by
where it is called, not by the Cargo section it is listed under.
