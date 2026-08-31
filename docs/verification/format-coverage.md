# Format Coverage Matrix

| Capability | Microsoft spec | PST M1 | MSG | Normalized model | Verified |
|---|---:|---:|---:|---:|---:|
| Open PST | Yes | Yes | N/A | N/A | Yes (P3, `tsp-tester.pst`) |
| Message store | Yes | Yes | N/A | Planned | Yes (P3) |
| Folder hierarchy | Yes | Yes | N/A | Planned | Yes (P3: 9 folders) |
| Message inventory | Yes | Yes | N/A | Planned | Yes (P3: 49 messages, 0 errors) |
| Message properties | Yes | Partial/raw | N/A | Planned | Yes (P3: 3,674 property values) |
| Message class | Yes | Aggregate counts only | N/A | Planned | Implemented (P4a), pending run |
| Plain body | Yes | Presence only | N/A | Planned | Implemented (P4a), pending run |
| HTML body | Yes | Presence only | N/A | Planned | Implemented (P4a), pending run |
| RTF body | Yes | Presence only | N/A | Planned | Implemented (P4a), pending run |
| Recipients (count) | Yes | Aggregate counts only | N/A | Planned | Implemented (P4a), pending run |
| Recipients (To/CC/BCC) | Yes | Not yet surfaced | N/A | Planned | Pending — needs column-level read |
| Attachments (count) | Yes | Aggregate counts only | N/A | Planned | Implemented (P4a), pending run |
| Attachments (content/classification) | Yes | Not yet surfaced | N/A | Planned | Pending — needs column-level read |
| Embedded messages | Yes | Not yet surfaced | N/A | Planned | Pending |
| Named properties | Yes | Dependency capability to be evaluated | N/A | Planned | Pending |
| Markdown | Application requirement | No | No | Planned | Pending |
| Attachment archive | Application requirement | No | No | Planned | Pending |

"Implemented (P4a), pending run" means the code exists and is believed
correct against the documented `outlook-pst` v1.2.0 API, but has not yet been
compiled or run on the Windows quality-gate/fixture, because the drafting
environment has no Rust toolchain or PST access. Windows quality-gate output
plus a `tsp-tester.pst` run are required before these rows can move to
"Verified".
