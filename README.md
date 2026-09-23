# teaspoon (tsp)

Standalone Rust tooling for deterministic, loss-aware mining of Outlook `.pst` and `.msg` files.

Shorthand for teaspoon (i.e. the name of this project) is tsp (i.e. the name of this project's tool), which is "pst" backwards. (ツ)

## Current status

**M0 — architecture/research baseline:** established. Five of six ADRs are Accepted; ADR-0006 (independent differential verification) remains Proposed until that verification work actually happens.

**M1 — PST feasibility spike: complete**, within the limits of `outlook-pst` v1.2.0's public API. A small read-only CLI (`tsp`) exercises the Microsoft Rust PST implementation: open a PST, reach the message store/IPM subtree, traverse folders, enumerate messages, and inspect raw message properties, plus aggregate message-class, body-availability, recipient-count/type, and attachment-count/classification diagnostics — all without emitting any message content.

- P1 through P4b are done and verified on Windows against a real PST fixture deliberately enhanced to cover plain/RTF bodies, a BCC recipient, an embedded-message attachment, and an OLE attachment — see `docs/verification/m1-results.md`.
- Zero-byte and by-reference attachments were explicitly excluded as fixture goals (empirically impractical to compose / obsolete in modern email) — see `docs/verification/m1-results.md` for the documented rationale. The classification code for both remains.
- Opening/traversing embedded-message or OLE attachment *content* was investigated (P4c) and found not achievable through `outlook-pst` v1.2.0's public API — accepted as M1's practical ceiling, not a defect. `tsp` correctly detects and counts these attachments; it can't open them.
- HTML-in-RTF detection (MS-OXRTFEX `\fromhtml1` encapsulation) is implemented and confirmed correct against real data — see `docs/verification/m1-results.md`.

This is deliberately **not** the production miner and does not yet emit Markdown or extract body/attachment content.

**M2 — MSG ingestion spike: body-type detection, recipient/attachment classification, and the zero-byte/subdirectory-visibility fixes are all confirmed correct against real data.** `tsp` dispatches on its input: a `.pst` file uses the unchanged M1 path; a single `.msg` file or a directory of `.msg` files (scanned non-recursively) uses a diagnostic built on the `msg_parser` crate, mirroring M1's structure. Opening an embedded-message attachment as a nested message (M2c) was investigated across two independent real attempts and found not achievable in practice, for a root cause not identified despite repeated research — an accepted ceiling, mirroring P4c on the PST side. See `docs/verification/m2-results.md` for the full evidence trail. `msg_parser` remains a provisional choice, not a final production commitment — see the custom-parser groundwork below.

**Custom MS-OXMSG parser groundwork (experimental, opt-in): structural enumeration and property-entry decoding both verified against real data; value content is deliberately not yet extracted.** `msg_parser` has no raw/generic property-iteration equivalent to `outlook-pst`'s `.get(id)`/`.iter()` — the one structural inconsistency remaining between teaspoon's two format adapters, and directly at odds with the loss-aware-representation principle below. Run with `--oxmsg` against `.msg` input to use a from-scratch structural enumeration built on the `cfb` crate (a generic MS-CFB/Compound File Binary reader) instead of `msg_parser`. It opens a real `.msg` container, enumerates every entry with zero unrecognized (M2.x), decodes every property entry's type/ID/flags, reads variable-length value streams (size and encoding validated, content not extracted), and resolves named properties to their GUID set and numeric/string identity. It does not yet decode a property's actual value, only its structural shape. See `docs/verification/oxmsg-results.md`.

## Design principles

1. `.pst` and `.msg` are input formats, not the domain model.
2. Format adapters produce a common Outlook-item representation.
3. Markdown is a projection, not the canonical representation.
4. Unknown/unmapped properties are preserved or reported rather than silently discarded.
5. Extraction loss is explicit.
6. Source provenance is part of the output model.
7. Evidence distinguishes specification support, implementation support, tests, and independent verification.

## Usage

```powershell
cargo run --release -- .\sample.pst
cargo run --release -- .\sample.msg
cargo run --release -- .\folder-of-msgs\
cargo run --release -- --oxmsg .\sample.msg   # experimental custom MS-OXMSG parser
```

Real PST/MSG fixtures are required to perform the behavioral portion of any milestone. No personal mail data is embedded in this repository.

## Important limitation

Neither M1 nor M2 yet proves complete extraction fidelity. Markdown rendering, attachment byte preservation, named-property normalization (structural resolution now works on the MSG side; nothing yet feeds the normalized model), body extraction, and differential validation remain future work.
