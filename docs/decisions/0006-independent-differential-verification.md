# ADR-0006: Independent differential verification

- Status: Proposed
- Date: 2026-08-29

## Decision

Use independent implementations where practical to verify extraction behavior.

Candidate PST oracles include libpff and libpst. The Rust `msg_parser` project is an initial MSG comparison candidate.

Agreement with the same implementation that produced the result is not considered independent verification.
