# ADR-0005: Deterministic Markdown archive

- Status: Accepted
- Date: 2026-08-29

## Decision

Each mined message will have a deterministic archive location containing:
- `message.md`
- `metadata.json`
- `attachments/`

Markdown is a projection of the normalized model, not the canonical data model.

## Consequences

Message identity, filename sanitization, collision handling, and attachment relationship semantics must be specified before production export.
