# ADR-0003: Standalone Rust execution

- Status: Proposed
- Date: 2026-08-29

## Decision

The production miner will not require Classic Outlook, MAPI, COM, or Office at runtime.

Outlook may be retained on a development Windows machine as an independent validation oracle.

## Consequences

The parser and archive must be self-contained and deterministic from input files.
