# M1 PST Feasibility Spike

## Status

**P1 through P4b are complete and verified on Windows against a real PST
fixture that was deliberately enhanced (v0.1.4.3) to close most of the
representativeness gaps identified in the original `tsp-tester.pst`.** Two
minor gaps remain (see below). No code changes were required to observe
this new evidence — the enhancement was to the fixture, not the code.

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

## Full verified evidence (enhanced `tsp-tester.pst`, v0.1.4.3)

```text
ipm_subtree=ok
inventory=privacy_safe
ipm_subtree=opened
folders=10
messages=57
message_open_errors=0
folder_open_errors=0
property_values=4096
message_class_read_errors=0
message_class class=IPM.Note count=57
bodies_plain=1
bodies_html=55
bodies_rtf=1
messages_with_recipients=57
total_recipients=69
max_recipients_on_a_message=6
messages_with_attachments=25
total_attachments=79
max_attachments_on_a_message=16
recipient_row_read_errors=0
recipients_orig=0
recipients_to=57
recipients_cc=11
recipients_bcc=1
recipients_type_other=0
recipients_type_unknown=0
attachment_row_read_errors=0
attachments_zero_byte=0
attachments_with_content_id=2
attachments_method_none=0
attachments_method_by_value=77
attachments_method_by_reference=0
attachments_method_by_reference_resolve=0
attachments_method_by_reference_only=0
attachments_method_embedded_message=1
attachments_method_ole=1
attachments_method_other=0
attachments_method_unknown=0
```

**Internal consistency, again confirmed against the enhanced fixture:**
body-presence counts sum to message count (1+55+1=57); recipient-type
buckets sum to `total_recipients` (57+11+1=69); attachment-method buckets
sum to `total_attachments` (77+1+1=79). All three independent cross-checks
hold exactly.

## What the fixture enhancement closed

| Dimension | v0.1.4.2 (original fixture) | v0.1.4.3 (enhanced fixture) | Status |
|---|---:|---:|---|
| Plain-text-only body | 0/49 | 1/57 | **Closed** |
| RTF-only body | 0/49 | 1/57 | **Closed** |
| BCC recipient | 0 | 1 | **Closed** |
| Embedded-message attachment | 0 | 1 | **Closed** |
| OLE attachment | 0 | 1 | **Closed** |
| Zero-byte attachment | 0 | 0 | Still open |
| By-reference attachment (any of the 3 sub-methods) | 0 | 0 | Still open |

5 of the 7 gaps identified after P4b are now closed with real evidence,
using the same code — no code changes were made between v0.1.4.2 and
v0.1.4.3, only the fixture changed.

## New opportunity this unlocks

With a real `attachments_method_embedded_message=1` now in the fixture,
**opening and traversing that embedded message as a nested message is now
a testable capability**, whereas before it was explicitly "unreachable with
current fixture." This was not previously buildable against real data; it
is now. See "Next best action" framing in project correspondence for
whether/when to pursue this (P4c candidate) — not decided here.

## What remains unproven

No claim is made that teaspoon has proven:
- all PST variants;
- complete property fidelity;
- body *extraction* (only body-type *availability* is established);
- attachment *extraction* (only attachment *counts and classification
  counts* are established — never attachment bytes);
- opening/traversing the embedded message now known to exist in the
  fixture (classification counts it; nothing opens it yet);
- reading OLE attachment content (classification counts it; nothing reads
  it yet);
- zero-byte attachment handling against a real row (code exists, fixture
  still has none);
- by-reference attachment handling (any of the 3 sub-methods) against a
  real row (code exists, fixture still has none);
- reliable inline-image detection (content-ID presence is a heuristic,
  weakly exercised: 2/79 attachments);
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation;
- MSG support.

Those require further fixtures, further implementation, and explicit tests.
