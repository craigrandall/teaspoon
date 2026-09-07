# M1 PST Feasibility Spike

## Status

**P1 (naming), P2 (privacy-safe diagnostics), P3 (behavioral run), P4a
(extended aggregate diagnostics), and P4b (recipient-type and attachment
classification) are all complete and verified on Windows against a real
PST fixture.** No further coding work is required to close M1's read-side
inventory capability against `tsp-tester.pst` specifically; what remains is
a fixture-adequacy decision (see below) and, separately, differential
verification (ADR: independent-differential-verification, not yet started).

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

## Full verified evidence (`tsp-tester.pst`, v0.1.4.2)

```text
ipm_subtree=ok
inventory=privacy_safe
ipm_subtree=opened
folders=9
messages=49
message_open_errors=0
folder_open_errors=0
property_values=3674
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
recipient_row_read_errors=0
recipients_orig=0
recipients_to=49
recipients_cc=10
recipients_bcc=0
recipients_type_other=0
recipients_type_unknown=0
attachment_row_read_errors=0
attachments_zero_byte=0
attachments_with_content_id=2
attachments_method_none=0
attachments_method_by_value=76
attachments_method_by_reference=0
attachments_method_by_reference_resolve=0
attachments_method_by_reference_only=0
attachments_method_embedded_message=0
attachments_method_ole=0
attachments_method_other=0
attachments_method_unknown=0
```

Verified on Windows 11: `cargo fmt --check && cargo check && cargo clippy
--all-targets --all-features -- -D warnings && cargo test && cargo build
--release` all pass; 9/9 unit tests pass.

**Internal consistency check:** the P4b recipient-type buckets
(orig+to+cc+bcc+other+unknown = 0+49+10+0+0+0 = 59) sum exactly to P4a's
independently-computed `total_recipients` (59). The P4b attachment-method
buckets (0+76+0+0+0+0+0+0+0 = 76) sum exactly to P4a's independently-computed
`total_attachments` (76). Since these come from two different code paths
(`rows_matrix().count()` vs. per-row column classification), this is
corroborating evidence of correctness, not merely a passing compile.

## Representativeness assessment (P4), updated with P4b evidence

| Dimension | Verdict |
|---|---|
| Folder/message traversal | Strong — 9 folders, 49 messages, zero errors |
| Message class | Appropriate — pure `IPM.Note`, matches v1 email-only scope |
| Body formats | **Gap** — every message is HTML-only; no plain-only or RTF-only messages observed |
| Recipients (count) | Good — present on all 49 messages, up to 6 on one message |
| Recipients (type) | **Partial** — To and CC both exercised (49 To, 10 CC); **zero BCC, zero ORIG** observed. BCC absence may be inherent to how sent-copy PSTs retain recipient data rather than a fixture defect — not yet determined either way. |
| Attachments (count) | Strong — 22/49 messages (45%) carry attachments, 76 total, up to 16 on one message |
| Attachments (method) | **Gap** — all 76 attachments are `by_value` (embedded directly in the message). Zero by-reference, zero embedded-message, zero OLE. |
| Attachments (zero-byte) | Not exercised — zero found; can't confirm the zero-byte code path against a real zero-byte attachment |
| Attachments (inline heuristic) | Weakly exercised — only 2/76 attachments carry a Content-ID |

**Net assessment:** `tsp-tester.pst` has now proven every currently-implemented
read path can run cleanly end-to-end, but it does not exercise several
dimensions the project's own fixture-design goals called for: plain/RTF
bodies, BCC recipients, and non-`by_value` attachment storage (by-reference,
embedded messages, OLE). Whether to accept this as sufficient for now,
or to source/construct a second fixture before further extraction work,
is open and not yet decided — see "Next best action" framing in project
correspondence rather than assumed here.

## What remains unproven

No claim is made that teaspoon has proven:
- all PST variants;
- complete property fidelity;
- body *extraction* (only body-type *availability* is established, and only
  for HTML — plain/RTF availability detection is implemented but has never
  matched a real message);
- attachment *extraction* (only attachment *counts and classification
  counts* are established — never attachment bytes);
- by-reference, embedded-message, or OLE attachment handling (the
  classification code exists and compiles, but has never matched a real
  row of any of those three methods);
- zero-byte attachment handling (same: implemented, never matched a real
  row);
- reliable inline-image detection (P4b's content-ID presence check is a
  heuristic, and only 2 real attachments have exercised it at all);
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation;
- MSG support.

Those require further fixtures, further implementation, and explicit tests.
