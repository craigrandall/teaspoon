# M1 PST Feasibility Spike

## Status

**P1 (naming), P2 (privacy-safe diagnostics), and P3 (behavioral run against a real
PST) are complete. P4a (extended aggregate diagnostics) is implemented pending
Windows quality-gate and fixture-run verification.**

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
   attachment-count diagnostics (P4a).

## P3 behavioral evidence (real fixture)

`tsp.exe` was run against `tsp-tester.pst` (~24.4 MB, real-world fixture) using
the P2 privacy-safe build:

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

This is the first behavioral evidence that the PST dependency can open a real
PST, reach the IPM subtree, traverse a 9-folder hierarchy, and open all 49
discovered messages without error, while enumerating 3,674 property values
across those messages.

It does **not** establish that teaspoon can extract bodies, recipients, or
attachments — those required the P4a extension below, and P4a's own output
against this same fixture is still outstanding pending a Windows quality-gate
run.

## P4a: extended aggregate diagnostics (implemented, unverified pending Windows run)

To assess `tsp-tester.pst`'s representativeness as an M1 fixture, the
diagnostic was extended to report, still without emitting any message
content:

- `message_class_read_errors`, and one `message_class class=<name> count=<n>`
  line per distinct MAPI message class observed (message class names, e.g.
  `IPM.Note`, are a bounded standard MAPI vocabulary, not user-authored
  content, so reporting them by name does not violate the privacy boundary);
- `bodies_plain`, `bodies_html`, `bodies_rtf` — counts of messages where
  `PidTagBody` (0x1000), `PidTagBodyHtml` (0x1013), or `PidTagRtfCompressed`
  (0x1009) are *present*, never their contents;
- `messages_with_recipients`, `total_recipients`,
  `max_recipients_on_a_message` — recipient **counts** only, from
  `Message::recipient_table()`;
- `messages_with_attachments`, `total_attachments`,
  `max_attachments_on_a_message` — attachment **counts** only, from
  `Message::attachment_table()`.

### Deliberately deferred in this pass

- **Recipient type breakdown (To/CC/BCC).** `PidTagRecipientType` is a
  per-row column on the recipient table, not a `Message`-level accessor.
  Reading it requires locating the column via `TableContextInfo::columns()`
  and reading it per row via `TableContext::read_column()`. This is
  implementable but was not attempted in this pass without first validating
  the column-lookup path against a real fixture.
- **Attachment classification (zero-byte / embedded-message / inline).**
  Same constraint: `PidTagAttachSize` and `PidTagAttachMethod` are per-row
  attachment-table columns, not `Message`-level accessors.

Both are recorded as open follow-on work rather than attempted speculatively,
consistent with the project's evidence-first discipline: guessing at the
column-lookup API risked repeating the earlier failed P4a attempt that
assumed API shapes instead of reading them.

## What remains unproven

No claim is made that teaspoon has proven:
- all PST variants;
- complete property fidelity;
- body *extraction* (only body-type *availability* is established);
- attachment *extraction* (only attachment *counts* are established);
- recipient type (To/CC/BCC) classification;
- embedded messages;
- inline image / Content-ID handling;
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation;
- MSG support.

Those require further fixtures, further implementation, and explicit tests.
