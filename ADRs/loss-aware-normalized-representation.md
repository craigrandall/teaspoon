---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Loss-aware normalized representation

## Decision

The normalized representation will retain raw/unknown properties where practical and will record extraction diagnostics.

Extraction status must distinguish at least:
- complete
- partial
- failed

Unsupported or externally referenced attachment content must never be silently reported as preserved.
