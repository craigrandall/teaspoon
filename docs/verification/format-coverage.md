# Format Coverage Matrix

| Capability | Microsoft spec | PST M1 | MSG | Normalized model | Verified |
|---|---:|---:|---:|---:|---:|
| Open PST | Yes | Yes | N/A | N/A | Yes (P3, `tsp-tester.pst`) |
| Message store | Yes | Yes | N/A | Planned | Yes (P3) |
| Folder hierarchy | Yes | Yes | N/A | Planned | Yes (P3: 9 folders) |
| Message inventory | Yes | Yes | N/A | Planned | Yes (P3: 49 messages, 0 errors) |
| Message properties | Yes | Partial/raw | N/A | Planned | Yes (P3: 3,674 property values) |
| Message class | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: `IPM.Note`=49) |
| Plain body | Yes | Presence only | N/A | Planned | Yes (P4a: 0/49 — fixture gap) |
| HTML body | Yes | Presence only | N/A | Planned | Yes (P4a: 49/49) |
| RTF body | Yes | Presence only | N/A | Planned | Yes (P4a: 0/49 — fixture gap) |
| Recipients (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 59 across 49 messages) |
| Recipients (To/CC/BCC) | Yes | Per-row classification | N/A | Planned | Implemented (P4b), pending run |
| Attachments (count) | Yes | Aggregate counts only | N/A | Planned | Yes (P4a: 76 across 22 messages) |
| Attachments (zero-byte) | Yes | Per-row classification | N/A | Planned | Implemented (P4b), pending run |
| Attachments (method: by-value/by-ref/embedded/OLE) | Yes | Per-row classification | N/A | Planned | Implemented (P4b), pending run |
| Attachments (inline heuristic) | Yes | Content-ID presence only | N/A | Planned | Implemented (P4b), pending run |
| Attachment content (bytes) | Yes | Not yet surfaced | N/A | Planned | Pending |
| Embedded messages (opened/traversed) | Yes | Not yet surfaced | N/A | Planned | Pending — P4b counts them via method=5, doesn't open them |
| Named properties | Yes | Dependency capability to be evaluated | N/A | Planned | Pending |
| Markdown | Application requirement | No | No | Planned | Pending |
| Attachment archive | Application requirement | No | No | Planned | Pending |
| Colored/highlighted text preservation | Application requirement | No | No | Design question open (M4) | Pending — see docs/verification/m1-results.md and ADR backlog |

"Implemented (P4b), pending run" means the code exists and is believed
correct against the `outlook-pst` v1.2.0 API as confirmed directly from the
crate's own `cargo doc` output, but has not yet been compiled or run on the
Windows quality-gate/fixture, because the drafting environment has no Rust
toolchain or PST access. A Windows quality-gate run plus a `tsp-tester.pst`
run are required before these rows can move to "Yes".
