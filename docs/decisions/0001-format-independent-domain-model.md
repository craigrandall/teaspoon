# ADR-0001: Format-independent Outlook domain model

- Status: Proposed
- Date: 2026-08-29

## Context

PST and MSG are different physical formats but serialize Outlook/MAPI Message, Recipient, Attachment and Property concepts.

## Decision

PST and MSG ingestion adapters will produce a common normalized Outlook-item model.

The renderer and archive writer will not depend on PST or MSG types.

## Consequences

- The same Markdown/archive pipeline handles both formats.
- Format-specific parsing remains isolated.
- The model must retain raw/unknown properties where feasible.
- Some format-specific provenance remains necessary.
