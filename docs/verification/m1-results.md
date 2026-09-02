# M1 PST Feasibility Spike

## Status

**P1 (naming), P2 (privacy-safe diagnostics), P3 (behavioral run against a
real PST), and P4a (extended aggregate diagnostics) are complete and
verified on Windows.** P4b (recipient-type and attachment classification)
is implemented against the confirmed `outlook-pst` v1.2.0 API, pending a
Windows quality-gate and fixture-run.

## What M1 establishes

The spike uses the Microsoft Rust PST implementation (`outlook-pst` v1.2.0,
`github.com/microsoft/outlook-pst-rs`) rather than reimplementing PST parsing.

The path exercised is:

1. open PST;
2. obtain the IPM subtree entry ID;
3. open the IPM subtree;
4. traverse folder hierarchy;
5. traverse folder contents;
6. open each message;
7. inspect its raw property collection;
8. report deterministic inventory totals (P2);
9. report aggregate message-class, body-availability, recipient-count, and
   attachment-count diagnostics (P4a);
10. report recipient-type (To/CC/BCC) and attachment classification
    (zero-byte / method / inline-candidate) diagnostics (P4b).

## P3 behavioral evidence (real fixture, v0.1.0)

`tsp.exe` was run against `tsp-tester.pst` (~24.4 MB, real-world fixture)
using the P2 privacy-safe build:

```text
ipm_subtree=ok
inventory=privacy_safe
ipm_subtree=opened
folders=9
messages=49
message_open_errors=0
folder_open_errors=0
property_values=3674
```

This is the first behavioral evidence that the PST dependency can open a
real PST, reach the IPM subtree, traverse a 9-folder hierarchy, and open
all 49 discovered messages without error, while enumerating 3,674 property
values across those messages.

## P4a evidence (real fixture, v0.1.1)

Extended to report, still without emitting any message content:

```text
message_class_read_errors=0
message_class class=IPM.Note count=49
bodies_plain=0
bodies_html=49
bodies_rtf=0
messages_with_recipients=49
total_recipients=59
max_recipients_on_a_message=6
messages_with_attachments=22
total_attachments=76
max_attachments_on_a_message=16
```

Verified on Windows 11 (`cargo fmt && cargo check && cargo clippy
--all-targets --all-features -- -D warnings && cargo test && cargo build
--release`, all pass; 5/5 unit tests pass) and against `tsp-tester.pst`.

**Representativeness assessment (P4):**

| Dimension | Verdict |
|---|---|
| Folder/message traversal | Strong — 9 folders, 49 messages, zero errors |
| Message class | Appropriate — pure `IPM.Note`, matches v1 email-only scope |
| Body formats | Gap — every message is HTML-only; no plain-only or RTF-only messages observed |
| Recipients | Good — present on all 49 messages, up to 6 on one message |
| Attachments | Strong — 22/49 messages (45%) carry attachments, 76 total, up to 16 on one message |

`tsp-tester.pst` does not exercise plain-text-only or RTF-only message
bodies. Whether a second fixture is needed to cover that gap is open
research, not yet decided.

## P4b: recipient type and attachment classification (implemented, v0.1.3, unverified pending Windows run)

The `outlook-pst` v1.2.0 column-read API was confirmed directly from the
crate's own `cargo doc` output (`TableContext::context()` →
`TableContextInfo::columns()` → `[TableColumnDescriptor]`, per-row values via
`TableRowData::columns()`, and `TableContext::read_column()` to decode a
`TableRowColumnValue` into a `PropertyValue`), not guessed from the earlier
docs.rs research pass, which had reached its limit on this specific API
surface.

Added, still without emitting any name, address, or filename content:

- `recipients_orig`, `recipients_to`, `recipients_cc`, `recipients_bcc`,
  `recipients_type_other`, `recipients_type_unknown`,
  `recipient_row_read_errors` — from `PidTagRecipientType` (0x0C15) per
  recipient row;
- `attachments_zero_byte` — from `PidTagAttachSize` (0x0E20) == 0;
- `attachments_method_none` / `_by_value` / `_by_reference` /
  `_by_reference_resolve` / `_by_reference_only` / `_embedded_message` /
  `_ole` / `_other` / `_unknown`, `attachment_row_read_errors` — from
  `PidTagAttachMethod` (0x3705);
- `attachments_with_content_id` — *presence only* of `PidTagAttachContentId`
  (0x3712), a common but not definitive signal of an inline-referenced
  attachment (e.g. an inline image). This is a heuristic, not a MAPI-defined
  "is inline" flag.

## What remains unproven

No claim is made that teaspoon has proven:
- all PST variants;
- complete property fidelity;
- body *extraction* (only body-type *availability* is established);
- attachment *extraction* (only attachment *counts and classification
  counts* are established — never attachment bytes);
- embedded messages (counted via P4b's attachment-method classification,
  but not yet opened/traversed as nested messages);
- reliable inline-image detection (P4b's content-ID presence check is a
  heuristic, not a definitive classification);
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation;
- MSG support.

Those require further fixtures, further implementation, and explicit tests.
