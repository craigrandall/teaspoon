# Format Coverage Matrix

| Capability | Microsoft spec | PST M1 | MSG | Normalized model | Verified |
|---|---:|---:|---:|---:|---:|
| Open PST | Yes | Yes | N/A | N/A | Yes (P3, `tsp-tester.pst`) |
| Message store | Yes | Yes | N/A | Planned | Yes (P3) |
| Folder hierarchy | Yes | Yes | N/A | Planned | Yes (P3: 9 folders) |
| Message inventory | Yes | Yes | N/A | Planned | Yes (P3: 49 messages, 0 errors) |
| Message properties | Yes | Partial/raw | N/A | Planned | Yes (P3: 3,674 property values) |
| Message class | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: `IPM.Note`=49) |
| Plain body | Yes | Presence only | N/A | Planned | Code path exists; never matched a real message (0/49 in fixture) |
| HTML body | Yes | Presence only | N/A | Planned | Yes (P4a: 49/49) |
| RTF body | Yes | Presence only | N/A | Planned | Code path exists; never matched a real message (0/49 in fixture) |
| Recipients (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 59 across 49 messages) |
| Recipients (To/CC) | Yes | Per-row classification | N/A | Planned | Yes (P4b: 49 To, 10 CC) |
| Recipients (BCC) | Yes | Per-row classification | N/A | Planned | Code path exists; never matched a real row (0 in fixture) |
| Attachments (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 76 across 22 messages) |
| Attachments (by-value) | Yes | Per-row classification | N/A | Planned | Yes (P4b: 76/76) |
| Attachments (by-reference / embedded-message / OLE) | Yes | Per-row classification | N/A | Planned | Code path exists; never matched a real row (0 of each in fixture) |
| Attachments (zero-byte) | Yes | Per-row classification | N/A | Planned | Code path exists; never matched a real row (0 in fixture) |
| Attachments (inline heuristic) | Yes | Content-ID presence only | N/A | Planned | Weakly exercised (2/76 attachments) |
| Attachment content (bytes) | Yes | Not yet surfaced | N/A | Planned | Pending |
| Embedded messages (opened/traversed) | Yes | Not yet surfaced | N/A | Planned | Pending — and unreachable with current fixture (0 embedded-message attachments exist in it) |
| Named properties | Yes | Dependency capability to be evaluated | N/A | Planned | Pending |
| Markdown | Application requirement | No | No | Planned | Pending |
| Attachment archive | Application requirement | No | No | Planned | Pending |
| Colored/highlighted text preservation | Application requirement | No | No | Design question open (M4) | Pending — see docs/verification/m1-results.md and ADR backlog |

"Code path exists; never matched a real row/message" means the
classification logic is implemented, unit-tested against synthetic inputs,
and compiles/runs cleanly, but `tsp-tester.pst` contains zero real instances
of that case, so the code's behavior against genuine PST data in that
branch is unconfirmed. This is a fixture-adequacy gap, not an
implementation gap.
