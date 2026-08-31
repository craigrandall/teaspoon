---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Deterministic Markdown archive

## Decision

Each mined message will have a deterministic archive location containing:
- `message.md`
- `metadata.json`
- `attachments/`

Markdown is a projection of the normalized model, not the canonical data model.

## Consequences

Message identity, filename sanitization, collision handling, and attachment relationship semantics must be specified before production export.
