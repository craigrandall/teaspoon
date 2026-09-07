# M1 PST Feasibility Spike

## Status

**Complete, within the limits of `outlook-pst` v1.2.0's public API.** P1
through P4b are done and verified on Windows against a real PST fixture
deliberately enhanced to close 5 of 7 identified representativeness gaps.
The remaining 2 (zero-byte and by-reference attachments) were explicitly
excluded as non-goals (see below). Opening/traversing embedded-message or
OLE attachment content was investigated for P4c and found to be beyond
what the dependency's public API supports (see "P4c" section below). This
is accepted as the practical ceiling of M1's PST read capability for now,
confirmed 2026-09-07. M1 is not blocked or failing — it has reached the
limit of what this specific dependency, used as intended, can prove.

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
| Zero-byte attachment | 0 | 0 | **Deliberately excluded from fixture goals — see rationale below** |
| By-reference attachment (any of the 3 sub-methods) | 0 | 0 | **Deliberately excluded from fixture goals — see rationale below** |

5 of the 7 gaps identified after P4b are now closed with real evidence,
using the same code — no code changes were made between v0.1.4.2 and
v0.1.4.3, only the fixture changed. The remaining 2 are excluded outright
(see below), so this fixture is now considered adequate against every
dimension this project set out to cover.

## Why zero-byte and by-reference attachments were dropped as fixture goals (2026-09-07)

Both were deliberately removed from the fixture-construction goal, not
merely left unattempted. This is a project decision, documented here so it
isn't mistaken for an oversight later.

**Zero-byte attachment.** Empirically demonstrated, with primary evidence,
that a literal 0-byte file cannot be turned into a genuine 0-byte
attachment through either of the two mail clients available for
constructing this fixture:

- Gmail refuses outright: attaching a confirmed 0-byte file produces the
  error "This file is 0 bytes, so it will not be attached."
- Classic Outlook accepts it but silently pads it: the same 0-byte source
  file appears as a 117-byte attachment once sent, because Outlook wraps
  every attachment in a MAPI/TNEF container (attachment method marker,
  filename, internal property headers) that has a non-zero minimum size
  even when the payload is empty — the same mechanism behind `winmail.dat`
  bloat.

This does not prove `PidTagAttachSize == 0` is impossible in every PST ever
produced (by-reference attachments, which store no byte content, or a
malformed/non-Outlook-authored PST, could still produce one). It does
establish that constructing this case through normal composition is
impractical, and that it is not representative of realistic
Outlook-authored content, which is what `tsp-tester.pst` is for. The
`attachments_zero_byte` classification code in `tsp` is retained (see
`record_attachment_size` in `src/main.rs`) as inexpensive, spec-correct
protection against any PST that does contain one — only the goal of
constructing an example for the fixture was dropped.

**By-reference attachment (any of `ATTACH_BY_REFERENCE`,
`ATTACH_BY_REFERENCE_RESOLVE`, `ATTACH_BY_REFERENCE_ONLY`).** This MAPI
attachment method was never exposed through standard compose UI in Outlook
or any modern webmail client; it required programmatic MAPI-level tooling
even when it was in active use. Modern "cloud attachment" functionality
(e.g. OneDrive links in Outlook) is implemented as a hyperlink/body-content
mechanism, not as a classic MAPI by-reference attachment, so it doesn't
produce this case either. Constructing a genuine example would require
MAPI-level tooling disproportionate to its relevance in realistic modern
data. The `attachments_method_by_reference*` classification code in `tsp`
is retained for the same reason as above — only the fixture-construction
goal was dropped.

## P4c: investigated and found not achievable via the public API (2026-09-07)

With a real `attachments_method_embedded_message=1` in the enhanced fixture,
opening and traversing that embedded message as a nested message became
testable for the first time, so it was investigated. The finding, traced
directly from `outlook-pst` v1.2.0's own `cargo doc` output:

- The `Attachment` trait exists (`ltp`-adjacent `messaging::attachment`
  module) with a `message(&self) -> Rc<dyn Message>` accessor — this is the
  intended embedded-message mechanism.
- The only way to construct an `Attachment` is
  `UnicodeAttachment::read(message: Rc<UnicodeMessage>, sub_node: NodeId,
  prop_ids) -> Result<Rc<Self>>` (and the `AnsiAttachment` equivalent for
  legacy PSTs) — both require the **concrete** `Rc<UnicodeMessage>` /
  `Rc<AnsiMessage>` type.
- `Store::open_message()` — the only way to obtain a message at all — is
  declared in the `Store` trait itself as returning `Result<Rc<dyn
  Message>>`. This holds regardless of whether it's called through a
  concrete `UnicodeStore`/`AnsiStore` or the type-erased `dyn Store`
  `outlook_pst::open_store()` returns; the trait method's signature fixes
  the return type.
- `Message`/`Store` have no `Any` supertrait or `as_any()` method, so
  there is no supported way to downcast `Rc<dyn Message>` back to a
  concrete type.

**Conclusion: `outlook-pst` v1.2.0's public API has no supported path from
"a message and its attachment table" to "an `Attachment` object with a
usable `.message()` accessor."** The `Attachment` trait and its concrete
implementations exist in the crate but aren't reachable through the
`Store`/`Folder`/`Message` workflow the rest of `tsp` uses. This is
accepted as a hard ceiling of the dependency's current public API, not
pursued further via unsafe workarounds or a lower-level (`ndb`/`ltp`)
reimplementation, both of which were considered and rejected as
disproportionate to what P4c set out to prove. `tsp` can *detect and
count* embedded-message and OLE attachments (P4b); it cannot open them.

This may be worth raising with the `outlook-pst-rs` maintainers as a
possible gap in a future crate version; not pursued as of this writing.

## What remains unproven

No claim is made that teaspoon has proven:
- all PST variants;
- complete property fidelity;
- body *extraction* (only body-type *availability* is established);
- attachment *extraction* (only attachment *counts and classification
  counts* are established — never attachment bytes);
- opening/traversing the embedded message known to exist in the fixture —
  investigated for P4c and found not achievable via `outlook-pst` v1.2.0's
  public API (see "P4c" above); classification still counts it correctly;
- reading OLE attachment content — same constraint as embedded messages;
- zero-byte attachment handling against a real row (code exists; fixture
  construction goal deliberately dropped — see rationale above);
- by-reference attachment handling, any of the 3 sub-methods, against a
  real row (code exists; fixture construction goal deliberately dropped —
  see rationale above);
- reliable inline-image detection (content-ID presence is a heuristic,
  weakly exercised: 2/79 attachments);
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation;
- MSG support.

Those require further fixtures, further implementation, and explicit tests.
