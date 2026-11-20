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
evidence that it has been performed for the MSG side and, since 2026-11-17,
for the PST envelope (against Classic Outlook).** The rest of the PST side
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

Later differential modes (MSG side, performed; all counts only, all against `msg_parser`, and so subject to the same two limits): `--verify-envelope` compares the subject, sender, and recipients (34 files, 0 mismatches on every comparable field; the fields `msg_parser` does not expose have no oracle); `--verify-deencap` compares the HTML the in-house RTF de-encapsulation recovers with `msg_parser`'s `html_from_rtf()` (recognition and presence agree on all 29 files; the recovered HTML differed in all 27 pairs, and every difference was traced to `msg_parser`, with one in-house limitation, a single Windows-1255 byte); and `--verify-attachments` (M4f-2c, 2026-10-20) compares the SHA-256 and length of every top-level by-value attachment, read through the export's own reader, with `msg_parser`'s payload (34 files: 30 of 30 match, 0 mismatches, gate 0). The last does not cover the attachments inside embedded messages, which `msg_parser` does not open, so those have no independent check. `--verify-bodies` is a content-preservation check of the converter, not a comparison with another implementation.

PST side (the envelope performed on 2026-11-17, v0.1.44; the rest outstanding): the PST envelope (M4i drop B) was compared with Classic Outlook's own reading of the same stores, the first PST-side comparison against an independent implementation. A PowerShell script reads the raw MAPI properties of each mail item through Outlook's `PropertyAccessor` and writes them to a local file; a comparer joins them to the exported `metadata.json` files on the node ID (the last four bytes of Outlook's 24-byte entry ID, with the 16-byte store identifier checked) and prints counts only. Over the 60 messages of `tsp-tester.pst` and the 44 messages of `crkb.pst` that Outlook reports with two or more recipients, 104 of 104 joined and every compared field agreed (the sender and sent-representing addresses, importance, sensitivity, conversation topic and index, transport headers, submit and delivery times to the second, and the To/Cc/Bcc lists with their order), `field_mismatches_total=0`. Two kinds of difference were classified as Outlook supplying what the PST does not store, and are counted separately rather than as agreement or as mismatch: a default sensitivity of 0 for a message that has none (36 and 11 messages), and the email address reported as the SMTP address of a recipient whose table has no SMTP column (48 and 24 recipients); `tsp` writes neither. The first run reported 120 time mismatches that were the comparison script's own local-time conversion and not the reader, the case of a defect in the oracle side that the "each discrepancy needs triage" consequence anticipated; the script now records both readings and the comparer says which one matched (the raw value is UTC). The limits: Outlook is a closed-source oracle and a second reader of the same file; the comparison read the copies mounted in Outlook, not the exported files; it covers the envelope only (no body, attachment, folder, or identity comparison); the `crkb.pst` run covered 44 messages, because the mounted copy holds 7,041 mail items against 6,508 exported and the full comparison was not run; and no comparison against `libpff` or `libpst` has been run, so the PST side outside the envelope remains outstanding. See [docs/verification/test-strategy.md](../docs/verification/test-strategy.md)
for where it sits in the overall evidence ladder.

## More Information

Candidate PST oracles: libpff, libpst (not yet integrated). Classic Outlook, driven through COM, has served as a PST oracle for the envelope since 2026-11-17 (the scripts are not part of the crate). MSG oracle in
use: the Rust `msg_parser` crate, through `--verify` only. It is a normal
Cargo dependency rather than a dev-dependency, because the verification
harness ships in the `tsp` binary; its oracle-only role is enforced by
where it is called, not by the Cargo section it is listed under.
