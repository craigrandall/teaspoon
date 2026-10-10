# Format Coverage Matrix

Two adapters are covered. **PST** is the M1 diagnostic over `outlook-pst`,
verified against the enhanced `tsp-tester.pst` (57 messages). **MSG** is the
custom MS-OXMSG parser (M3), the default `.msg` path since M3f, verified
against the 29-file `.msg` corpus and, for `msg_parser`-comparable fields,
differentially against `msg_parser` (see `m3-results.md`; the envelope fields
in `m4-results.md`). The normalized model exists only in part (message
content and, since M4d, the envelope); "Planned" in that column means the
rest of the model (M4b-4).

| Capability | Microsoft spec | PST (M1) | MSG (M3) | Normalized model | Verified |
|---|---:|---:|---:|---:|---:|
| Open container | Yes | Yes | Yes | N/A | PST: Yes (P3, `tsp-tester.pst`). MSG: Yes (29 files, 0 open errors; every CFB entry accounted for) |
| Message store | Yes | Yes | N/A (one message per file) | Planned | PST: Yes (P3) |
| Folder hierarchy | Yes | Yes | N/A (one message per file) | Planned | PST: Yes (P3: 10 folders) |
| Message inventory | Yes | Yes | Yes (file scan, non-recursive; skipped subdirectories reported) | Planned | PST: Yes (P3: 57 messages, 0 errors). MSG: Yes (29 files, 3 subdirectories skipped) |
| Message properties | Yes | Partial/raw | Decoded: fixed values to typed values; UTF-16LE, binary, CLSID, `PT_STRING8` value streams | Planned | PST: Yes (P3: 4,096 property values). MSG: Yes (3,251 property entries; every structural gate 0) |
| Message class | Yes | Aggregate counts only | Aggregate counts only | Planned | PST: Yes (P4a: `IPM.Note`=57). MSG: Yes (29 × `IPM.Note`, matches `msg_parser`) |
| Plain body | Yes | Presence only | Presence only; written verbatim to `message.md` by the export (M4c) | Yes (plain-text body) | PST: Yes (P4a: 1/57, v0.1.4.3). MSG: Yes (29/29, matches `msg_parser`); export: 34 of 34 exported messages had a plain-text body (v0.1.23.1) |
| HTML body (native `PidTagBodyHtml`) | Yes | Presence only | Presence only | Planned | PST: Yes — the 2026-09-13 fix ran on Windows: 55 native, 0 via RTF, 0 decompression errors. The suspected undercount of the earlier "55/57" figure did not materialize; the one RTF-only message is genuinely RTF-authored. MSG: 0 native in the corpus, matches `msg_parser` |
| HTML body (encapsulated in RTF, MS-OXRTFEX) | Yes | `\fromhtml1` marker detection only | `\fromhtml1` marker detection only (the same shared function) | Planned | PST: negative path confirmed on real data (the one RTF-only message is genuinely RTF-authored); positive path not yet confirmed on the PST side. MSG: 27/29 with the marker, matches `msg_parser`; 0 decompression errors. The in-house de-encapsulation of the HTML (M4e-1, `rtf_deencap.rs`) is built with unit and property tests; on the corpus it agrees with `msg_parser` on which 27 of 29 messages have HTML (the 10-token recognition rule agrees with the marker search on all 29), and every difference in the recovered HTML was traced to `msg_parser` (font-table text, injected `*`, dropped line breaks, Latin-1 for `\'hh`, `\uN` fallbacks, one dictionary-corrupted word); one Windows-1255 byte is replaced by U+FFFD |
| RTF body | Yes | Presence only | Presence only; decompressed per MS-OXRTFCP | Planned | PST: Yes (P4a: 1/57, v0.1.4.3). MSG: Yes (29/29). Decompressed size differs from `msg_parser`'s by −1 byte on 1 file of 29; the custom path is the one that matches the specification's dictionary |
| Recipients (count) | Yes | Aggregate counts only | Aggregate counts only | Planned | PST: Yes (P4a: 69 across 57 messages). MSG: Yes (29 messages with recipients, max 6 on one message, matches `msg_parser`) |
| Recipients (To/CC) | Yes | Per-row classification | Per-row classification | Planned | PST: Yes (P4b: 57 To, 11 CC). MSG: Yes (To 29, CC 6, matches `msg_parser`) |
| Recipients (BCC) | Yes | Per-row classification | Per-row classification | Planned | PST: Yes (P4b: 1, v0.1.4.3). MSG: Yes (1, matches `msg_parser`) |
| Recipients (ORIG / other / unresolved) | Yes | ORIG tracked | Tracked (`msg_parser` cannot see ORIG); the export counts non-To/Cc/Bcc rows as `recipients_unlisted` | Yes (count) | PST: 0 on fixture. MSG: 0 ORIG, 0 other, 0 unresolved on the corpus, `recipients_unlisted_total=0`; no real ORIG row exists to test against on either side |
| Recipient names and addresses (display name, address type, address, SMTP address) | Yes | Not surfaced | Extracted and written by the export (M4d); nothing derived or repaired | Yes (`Recipient`, `Address`) | MSG: Yes against `msg_parser` — 29 files, 36 recipients, names 36/36, emails 0 mismatches (60 of 65 person emails equal `PidTagEmailAddress`, 5 equal the SMTP property only, across senders and recipients); address-type property (0x3002) not covered by the oracle; the `EX`-without-SMTP path has no real instance (`exchange_without_smtp_total=0`) |
| Sender (name, address type, address, SMTP address) | Yes | Not surfaced | Extracted and written (M4d) | Yes | MSG: Yes against `msg_parser` — sender name 29/29, sender email 0 mismatches; present on 29/29 |
| Sent-representing ("on behalf of") | Yes | Not surfaced | Extracted and written (M4d) | Yes | MSG: present on 29/29; **no oracle**, values unchecked; identifiers 0x0064 and 0x0065 are tier C |
| Submit and delivery times | Yes | Not surfaced | Extracted as stored ticks plus one UTC string (M4d) | Yes | MSG: present on 29/29 each; **no oracle**, values unchecked |
| Importance and sensitivity | Yes | Not surfaced | Extracted as stored integers; labels for the standard values (M4d) | Yes | MSG: importance present 29/29, sensitivity 11/29; **no oracle**; label meanings from the MAPI headers, not a fetched page |
| Conversation topic and index | Yes | Not surfaced | Extracted (index as hex) into `metadata.json` (M4d) | Yes | MSG: present on 29/29 each; **no oracle** |
| Transport (Internet) headers | Yes | Not surfaced | Extracted verbatim into `metadata.json` (M4d) | Yes | MSG: present on 24/29; **no oracle** |
| Attachments (count) | Yes | Aggregate counts only | Aggregate counts only | Planned | PST: Yes (P4a: 79 across 25 messages). MSG: Yes (29 across 11 messages, max 11 on one message, matches `msg_parser`) |
| Attachments (by-value) | Yes | Per-row classification | Per-row classification | Planned | PST: Yes (P4b: 77/79). MSG: Yes (27/29, matches `msg_parser`) |
| Attachments (embedded-message) | Yes | Per-row classification | Per-row classification | Planned | PST: Yes, classification only (P4b: 1). MSG: Yes (1, matches `msg_parser`) |
| Attachments (OLE) | Yes | Per-row classification | Per-row classification | Planned | PST: Yes, classification only (P4b: 1). MSG: Yes (1, matches `msg_parser`) |
| Attachments (by-reference, any sub-method) | Yes | Per-row classification | Not separately surfaced (`attachments_method_other`=0 on the corpus) | Planned | PST: code exists; fixture-construction goal deliberately dropped (obsolete in modern composition — see m1-results.md). MSG: no real instance |
| Attachments (zero-byte) | Yes | Per-row classification | Per-row classification; only confirmed-empty streams count, unreadable ones are reported separately | Planned | PST: code exists; fixture goal dropped (empirically impractical to compose — see m1-results.md). MSG: Yes — 1 real zero-byte attachment, matches `msg_parser`; 2 zero-size non-by-value attachments counted separately |
| Attachments (inline heuristic) | Yes | Content-ID presence only | Content-ID presence only | Planned | PST: weakly exercised (2/79). MSG: 23/29 have a Content-ID, matches `msg_parser` |
| Attachment content (bytes) | Yes | Not yet surfaced | Read in memory to classify emptiness; not surfaced; the export records attachments as not extracted | Planned | Pending on both sides (M4f) |
| Embedded messages (opened as nested message) | Yes | Not achievable: `outlook-pst` v1.2.0's public API has no path from a message to an attachment object (P4c) | Yes, one level deep: opened through the attachment's own attach method; nested message class read | Planned | PST: ceiling accepted (P4c). MSG: Yes — 1 opened, 0 errors, class `IPM.Note`. `msg_parser` returns `None` for the same attachment |
| OLE attachment content | Yes | Not yet surfaced | Storage classified and counted; contents not interpreted (counted as opaque payload) | Planned | PST: pending, same constraint as embedded messages. MSG: the custom OLE storage is accounted for, not decoded |
| Named properties | Yes | Dependency capability to be evaluated | Resolved: property set and kind for every occurrence; names decoded in memory, never printed | Planned | PST: pending. MSG: Yes (436 occurrences resolved; both cross-check gates 0) |
| Code page–aware `PT_STRING8` decoding | Yes | N/A (handled by the dependency) | Chain: `PidTagMessageCodepage`, then `PidTagInternetCodepage`, then Windows-1252. Implements 1252, ISO-8859-1, US-ASCII, UTF-8; other code pages decode only all-ASCII input, otherwise reported as unsupported | Planned | MSG: chain resolution verified on real data (29 files: 3 from the message code page, 26 from the Internet code page, 0 fallback). Decoding: synthetic fixtures only, because the corpus has no `PT_STRING8` value |
| Structural accounting (every entry classified) | Yes | N/A | Yes; gates reported by `--verify` (`structural_gate_violations`), with a breakdown printed when a gate fires | N/A | MSG: 2,647 entries accounted, `entry_accounting_gap_total`=0, `unrecognized_entries_total`=0, `structural_gate_violations`=0 in the full-corpus `--verify` run (unchanged at v0.1.25.1) |
| Markdown | Application requirement | No | Yes, provisional (M4c, M4d): subject heading, envelope header list, plain-text body in a code fence; HTML and RTF bodies not converted | Partial | MSG: golden files (synthetic), and the real corpus exported at v0.1.23.1 (34 messages, reproducible tree hash, one message reviewed); at v0.1.25.1 a couple of messages examined, no corpus export reported. PST: not implemented |
| Attachment archive | Application requirement | No | No (attachments counted and recorded as not extracted) | Planned | Pending (M4f) |
| Colored/highlighted text preservation | Application requirement | No | No | Design question open (M4) | Pending — see docs/verification/m1-results.md and ADR backlog |

"Code path exists; never matched a real row/message" means the
classification logic is implemented, unit-tested against synthetic inputs,
and compiles/runs cleanly, but the fixture contains zero real instances of
that case, so the code's behavior against genuine data in that branch
is unconfirmed. This is a fixture-adequacy gap, not an implementation gap.

Several MSG rows rest on a single corpus of 29 files and, for comparable
fields, on `msg_parser` as the only oracle. "Matches `msg_parser`" is
agreement with an independent implementation, not proof of correctness; the
two differences that were found in M3 were resolved against the Microsoft
specifications (see `m3-results.md`). Rows marked "no oracle" have nothing
independent behind them yet beyond the specification and unit tests. The
envelope comparison covers the 29 top-level files only, not the 5 messages
in the subdirectories.
