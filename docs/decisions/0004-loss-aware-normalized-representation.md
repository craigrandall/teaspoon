# ADR-0004: Loss-aware normalized representation

- Status: Proposed
- Date: 2026-08-29

## Decision

The normalized representation will retain raw/unknown properties where practical and will record extraction diagnostics.

Extraction status must distinguish at least:
- complete
- partial
- failed

Unsupported or externally referenced attachment content must never be silently reported as preserved.
