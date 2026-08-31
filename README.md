# teaspoon (tsp)

Standalone Rust tooling for deterministic, loss-aware mining of Outlook `.pst` and `.msg` files.

Shorthand for teaspoon (i.e. the name of this project) is tsp (i.e. the name of this project's tool), which is "pst" backwards. (ツ)

## Current status

**M0 — architecture/research baseline:** established.

**M1 — PST feasibility spike:** a small read-only CLI is provided to exercise the Microsoft Rust PST implementation: open a PST, reach the message store/IPM subtree, traverse folders, enumerate messages, and inspect raw message properties.

This is deliberately **not** the production miner and does not yet emit Markdown.

## Design principles

1. `.pst` and `.msg` are input formats, not the domain model.
2. Format adapters produce a common Outlook-item representation.
3. Markdown is a projection, not the canonical representation.
4. Unknown/unmapped properties are preserved or reported rather than silently discarded.
5. Extraction loss is explicit.
6. Source provenance is part of the output model.
7. Evidence distinguishes specification support, implementation support, tests, and independent verification.

## M1 usage

```powershell
cargo run --release -- .\sample.pst
```

A real PST fixture is required to perform the behavioral portion of M1. No personal PST is embedded in this repository.

## Important limitation

M1 establishes the PST ingestion seam; it does **not** yet prove complete extraction fidelity. Markdown rendering, attachment byte preservation, named-property normalization, body extraction, and differential validation remain future work.
