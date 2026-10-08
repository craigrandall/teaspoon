# Verification Strategy

The project uses an evidence ladder inspired by the engineering discipline used in `ghlinks`.

## Levels

### T0 — build/static
- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets --all-features -- -D warnings`

### T1 — unit
Pure model, identity, path, renderer, and property-normalization behavior.

### T2 — fixture
Known PST/MSG fixtures with declared feature coverage.

### T3 — behavioral
Whole PST, folder, subtree, and individual-message selection semantics.

### T4 — fidelity
Compare normalized facts against independent implementations.

### T5 — differential
Compare PST behavior with an independent implementation such as libpff/libpst where practical, and MSG behavior with an independent parser.

### T6 — adversarial
Malformed, truncated, unusual, oversized, Unicode, duplicate-name, and unsupported-property cases.

### T7 — corpus
A representative real-world corpus with provenance and fixture metadata.

## Evidence rule

A capability is not considered "supported" merely because:
- the Microsoft specification defines it, or
- a dependency appears to implement it.

The project distinguishes:

1. Specified
2. Implemented
3. Unit-tested
4. Fixture-tested
5. Independently verified

## Where the project stands

| Level | PST | MSG |
|---|---|---|
| T0 build/static | CI on Linux and Windows | CI on Linux and Windows (last reported for `v0.1.23.1`; not reported for `v0.1.25.1`) |
| T1 unit | Yes (naming, planning, rendering) | Yes: classification, decoding (including the `PT_STRING8` code page chain), counters, every verify-comparison outcome, structural gates, the archive renderers against golden files (including an envelope-rich message), the envelope comparison rules, the export writer; 201 tests at `v0.1.26` |
| T2 fixture | `tsp-tester.pst` (57 messages, enhanced to cover 5 of 7 identified gaps) | 29-file `.msg` corpus (plus 3 subdirectories for the dry run and export), plus synthetic `.msg` files built at test time |
| T3 behavioral | Not started | Partial: export, consent, and overwrite behavior exercised on real output (v0.1.23.1) |
| T4 fidelity | Not started | Partial: field-by-field agreement with `msg_parser` on every comparable field, including (M4d) the subject, sender, and recipients of the 29 top-level files |
| T5 differential | Not started; no `libpff`/`libpst` comparison has been run | Done against `msg_parser` (`--verify`, re-run through `v0.1.25.1`; `--verify-envelope`, 0 mismatches; `--verify-deencap`, built, not yet run), two differences triaged against the specifications in M3; `libpff`/`libpst` not used |
| T6 adversarial | Not started | Partial: synthetic fixtures for unsupported code pages and undecodable strings; no malformed or truncated container corpus |
| T7 corpus | One PST | One 29-file corpus; fixtures deliberately not in the repository, and provenance is not recorded in it |

CI runs T0 and T1 only. It cannot run T2 or above, because real PST/MSG
files are excluded from the repository for privacy reasons. Those levels
are a manual step on a machine that has the fixtures.

## Keeping fixture-level evidence from being one-time evidence

A verification run against fixtures produces a recorded result. To make it a
regression gate, the same comparison should be re-runnable on demand. The
MSG side does this with a test that is gated on the corpus rather than
committed with it.

- `fixture_corpus_verify_is_clean` runs the same comparison `tsp --verify`
  prints, over the directory named by the `TSP_FIXTURE_DIR` environment
  variable. When the variable is unset it skips with a note, so plain
  `cargo test` stays hermetic and CI stays green without fixtures.
- It asserts zero mismatches on every comparable field and zero on every
  structural gate. The one tolerated difference is the documented RTF
  dictionary divergence, bounded to a fixed number of files so that a
  second divergence cannot hide inside it.
- Failure messages carry counts and gate names only, never content.
- Status: run against the 29-file corpus with the variable set, and passed
  (a run that names the test, not only a passing suite, since the test skips
  when the variable is unset). The `tsp --verify` output it asserts on has
  been recorded for the full corpus, with every mismatch and gate at 0 apart
  from the one tolerated RTF length difference. Nothing runs it
  automatically; it stays a manual step.
- Raising the tolerated RTF divergence, or adding any other exception,
  requires a specification-checked reason recorded in the M3 results.
- `--verify-envelope` (M4d) and `--verify-deencap` (M4e-1) have no corpus-gated test yet; they are run by hand
  and their output recorded in `m4-results.md`.

When a structural gate does fire, `--verify` prints the privacy-safe
structural breakdown after a `structural_breakdown=follows` marker, so the
same run that reports a violation also carries the evidence to triage it.

Design choices behind this, for the next fixture-gated check:

- Test the totals, not the printed text. The scan and the printing are
  separate functions, so assertions run on counters and are not coupled to
  output formatting.
- Prefer an environment variable to a committed path or a committed
  subset. Personal mail cannot be committed, and a scrubbed subset would
  no longer be the evidence it stands in for.
- Build small synthetic fixtures at test time for cases the corpus lacks
  (ANSI strings, unsupported code pages). They are hermetic and run in CI.
  They prove the decoder's logic, not that real producers behave that way.

## Regression and golden evidence in M4

- **`verify-split.ps1`** compares the default, `--verify`, and PST outputs of an earlier tag with those of the new build, byte for byte. It has been run for v0.1.20, v0.1.22, v0.1.23.1, and v0.1.25.1 (always IDENTICAL: 850, 2705, and 1092 bytes). It proves a change did not alter existing diagnostics; it says nothing about new output.
- **Golden files** (`tests/golden/`, synthetic messages, no personal data) pin the exact bytes of `message.md`, `metadata.json`, and an envelope-rich `message.md`. They run in CI.
- **Tree hash** (`export_tree_sha256`) lets two exports be compared at a glance; it changes whenever the output format changes, so each format change needs a new baseline from a real-corpus export.
- **Differential checks** compare against `msg_parser` only where it exposes the field; fields without an oracle are reported as presence counts and rest on the specification and unit tests. The tallies say which of two properties matched, but an absent property is compared as empty, so they cannot always separate "absent" from "present and different".

## M1 evidence target

M1 is intended to establish:
- the Microsoft Rust PST crate can be integrated;
- a PST can be opened without Outlook;
- the message store/IPM subtree can be reached;
- folder hierarchy can be traversed;
- folder contents can yield message entry IDs;
- messages can be opened and their property identifiers inspected.

It does not establish complete semantic extraction.

## M3 evidence target

M3 is intended to establish, for `.msg`:
- property values (fixed and variable-length) can be decoded, with every
  structural anomaly counted rather than dropped;
- named properties resolve;
- the custom parser agrees with an independent parser on every comparable
  output field, and every disagreement is triaged against the Microsoft
  specifications;
- the custom parser can become the default path with a re-runnable check
  behind it.

It does not establish complete semantic extraction, Markdown rendering, or
attachment byte preservation.

## M4 evidence target

M4 is intended to establish a deterministic, loss-explicit archive: names and
paths that are valid and unique on Windows, byte-identical re-exports, consent
before replacing anything, and every gap recorded. Each stage has its own
gate in `docs/plans/m4-plan.md` and its results in `m4-results.md`. As of
`v0.1.25.1` the evidence covers the planner, the `.msg` export with its
consent rules, and the envelope; it does not yet cover attachment bytes,
HTML and RTF body conversion, or PST export.
