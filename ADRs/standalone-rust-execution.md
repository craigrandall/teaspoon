---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Standalone Rust execution

## Context and Problem Statement

Reading PST/MSG content is possible either by parsing the container formats
directly, or by driving Classic Outlook through its MAPI/COM automation
interfaces on a Windows machine where Outlook is installed and configured.
The latter is well-trodden (many existing tools do this) but ties teaspoon's
runtime to a specific OS, a specific installed application, and a live
Outlook profile. Should the production miner depend on Outlook/MAPI/COM at
runtime, or must it be self-contained?

## Decision Drivers

- Desire to run `teaspoon` on any machine holding the PST/MSG files, not only
  ones with Outlook installed and profile-configured.
- Determinism: automation-driven extraction can be affected by Outlook
  application state (open profile, cached views, add-ins) in ways a direct
  file parser is not.
- Long-term maintainability: COM/MAPI automation is Windows-only and
  historically brittle across Outlook versions.

## Considered Options

- Standalone Rust: parse PST/MSG files directly, with no Classic
  Outlook/MAPI/COM dependency at runtime.
- Outlook COM/MAPI automation: drive a running, configured Outlook instance
  to enumerate and export items.
- A hybrid: use Outlook automation opportunistically for convenience, with
  direct parsing as a fallback.

## Decision Outcome

Chosen option: "Standalone Rust", because it is the only option that lets
teaspoon run cross-platform and deterministically from input files alone,
without requiring a licensed, installed, profile-configured copy of Outlook
on the host machine. Outlook may still be kept on a development Windows
machine, but only as an independent validation oracle, not as a runtime
dependency of the production miner.

### Consequences

- Good, because the parser and archive are self-contained and deterministic
  from input files, independent of any locally installed application state.
- Good, because it keeps the door open to running `teaspoon` on non-Windows
  hosts in the future.
- Bad, because `teaspoon` must implement its own PST/MSG parsing (or depend on
  a library that does), rather than delegating that work to Outlook's own,
  battle-tested parser.
- Bad, because any format edge case Outlook's own parser silently tolerates
  must be discovered and handled explicitly by `teaspoon` or its dependency,
  rather than being handled for free.

### Confirmation

M1's spike confirms `tsp.exe` opens and traverses a real PST
(`tsp-tester.pst`) on Windows 11 without Outlook running or a mail profile
configured, using only the `outlook-pst` crate. See
[docs/verification/m1-results.md](../docs/verification/m1-results.md).

## More Information

(Classic) Outlook, if locally available, is used only as a 
differential-verification oracle (see
[independent-differential-verification.md](independent-differential-verification.md)),
never as a runtime dependency of `tsp`.
