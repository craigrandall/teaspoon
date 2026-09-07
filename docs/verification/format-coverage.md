# Format Coverage Matrix

| Capability | Microsoft spec | PST M1 | MSG | Normalized model | Verified |
|---|---:|---:|---:|---:|---:|
| Open PST | Yes | Yes | N/A | N/A | Yes (P3, `tsp-tester.pst`) |
| Message store | Yes | Yes | N/A | Planned | Yes (P3) |
| Folder hierarchy | Yes | Yes | N/A | Planned | Yes (P3: 10 folders) |
| Message inventory | Yes | Yes | N/A | Planned | Yes (P3: 57 messages, 0 errors) |
| Message properties | Yes | Partial/raw | N/A | Planned | Yes (P3: 4,096 property values) |
| Message class | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: `IPM.Note`=57) |
| Plain body | Yes | Presence only | N/A | Planned | Yes (P4a: 1/57, v0.1.4.3) |
| HTML body | Yes | Presence only | N/A | Planned | Yes (P4a: 55/57) |
| RTF body | Yes | Presence only | N/A | Planned | Yes (P4a: 1/57, v0.1.4.3) |
| Recipients (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 69 across 57 messages) |
| Recipients (To/CC) | Yes | Per-row classification | N/A | Planned | Yes (P4b: 57 To, 11 CC) |
| Recipients (BCC) | Yes | Per-row classification | N/A | Planned | Yes (P4b: 1, v0.1.4.3) |
| Attachments (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 79 across 25 messages) |
| Attachments (by-value) | Yes | Per-row classification | N/A | Planned | Yes (P4b: 77/79) |
| Attachments (embedded-message) | Yes | Per-row classification | N/A | Planned | Yes, classification only (P4b: 1, v0.1.4.3) |
| Attachments (OLE) | Yes | Per-row classification | N/A | Planned | Yes, classification only (P4b: 1, v0.1.4.3) |
| Attachments (by-reference, any sub-method) | Yes | Per-row classification | N/A | Planned | Code path exists; never matched a real row (0 in fixture) |
| Attachments (zero-byte) | Yes | Per-row classification | N/A | Planned | Code path exists; never matched a real row (0 in fixture) |
| Attachments (inline heuristic) | Yes | Content-ID presence only | N/A | Planned | Weakly exercised (2/79 attachments) |
| Attachment content (bytes) | Yes | Not yet surfaced | N/A | Planned | Pending |
| Embedded messages (opened/traversed as nested message) | Yes | Not yet surfaced | N/A | Planned | Pending — now reachable: fixture has 1 real embedded-message attachment as of v0.1.4.3 |
| OLE attachment content | Yes | Not yet surfaced | N/A | Planned | Pending — now reachable: fixture has 1 real OLE attachment as of v0.1.4.3 |
| Named properties | Yes | Dependency capability to be evaluated | N/A | Planned | Pending |
| Markdown | Application requirement | No | No | Planned | Pending |
| Attachment archive | Application requirement | No | No | Planned | Pending |
| Colored/highlighted text preservation | Application requirement | No | No | Design question open (M4) | Pending — see docs/verification/m1-results.md and ADR backlog |

"Code path exists; never matched a real row/message" means the
classification logic is implemented, unit-tested against synthetic inputs,
and compiles/runs cleanly, but the fixture contains zero real instances of
that case, so the code's behavior against genuine PST data in that branch
is unconfirmed. This is a fixture-adequacy gap, not an implementation gap.
