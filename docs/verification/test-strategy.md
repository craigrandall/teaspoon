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

## M1 evidence target

M1 is intended to establish:
- the Microsoft Rust PST crate can be integrated;
- a PST can be opened without Outlook;
- the message store/IPM subtree can be reached;
- folder hierarchy can be traversed;
- folder contents can yield message entry IDs;
- messages can be opened and their property identifiers inspected.

It does not establish complete semantic extraction.
