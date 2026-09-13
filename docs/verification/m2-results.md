# M2 MSG Ingestion Spike

## Status

**Compiles and runs cleanly on Windows (15/15 tests pass) against a real 9-file `.msg` fixture set.** This is *not* the same as "verified" — the run surfaced three findings that need investigation before this diagnostic's output can be trusted (see "First real-run evidence and open questions" below). Treat the code as working; treat the specific numbers as under investigation, not confirmed.

## Dependency

`msg_parser` v0.3, chosen over building a custom MS-OXMSG parser for this
spike, per the comparative analysis in project correspondence
(2026-09-07): it has working support for opening embedded-message
attachments recursively (`Attachment::as_message()`), which is exactly the
capability P4c found unreachable in `outlook-pst`'s public API for the PST
side. This is a provisional choice for the spike, not a final production
commitment — see ADR backlog: the "MSG parser candidate" decision
(custom parser vs. `msg_parser` in production) remains explicitly deferred
to M3, when loss-accounting requirements for MSG are concrete.

`msg_parser` is a flattened, opinionated API (a single `Outlook` struct)
rather than `outlook-pst`'s raw MAPI-property-faithful model. This means
the MSG-side diagnostic below cannot currently report `*_read_errors`
counters with the same precision as the PST side's P4b (there is no
generic property-presence/failure signal equivalent to
`MessageProperties::get(id)`/`.iter()`) — this is a known, accepted
architectural asymmetry between the two format adapters for now.

## What this diagnostic checks

`tsp` now dispatches on the input path:
- a `.pst` file → the existing, unchanged PST diagnostic (M1);
- a single `.msg` file, or a directory containing `.msg` files (scanned
  non-recursively) → the new MSG diagnostic, aggregating across every file
  found, the same way the PST diagnostic aggregates across every message
  in a mailbox.

For each `.msg` file, still without printing any subject, address, body,
or filename content:

- message class (bounded MAPI vocabulary, same treatment as PST);
- body-type availability (plain / HTML / RTF, via non-empty-string checks
  on `body` / `html` / `rtf_compressed`);
- recipient counts, already split by type (`to` / `cc` / `bcc`) since
  `msg_parser` exposes these as structured `Vec<Person>` directly — no
  column-reading needed, unlike the PST side's P4b;
- attachment counts and classification (`attach_method`: by-value /
  embedded-message / OLE / other), zero-byte check, and Content-ID
  presence (inline-attachment heuristic, same caveat as the PST side);
- **for each embedded-message attachment: an actual attempt to open it**
  via `Attachment::as_message()`, one level deep, counting successes,
  failures, and the nested message's own class. This is the real test of
  whether `msg_parser`'s headline capability works against real data, not
  just documentation. Deeper recursion (opening an embedded message's own
  embedded messages) is explicitly not attempted in this pass.

## First real-run evidence and open questions (2026-09-07)

```text
inventory=privacy_safe
input_kind=msg
files_scanned=9
open_errors=0
message_class_missing=0
message_class class=IPM.Note count=9
bodies_plain=9
bodies_html=0
bodies_rtf=9
messages_with_recipients=9
recipients_to=9
recipients_cc=1
recipients_bcc=1
max_recipients_on_a_message=2
messages_with_attachments=4
total_attachments=4
max_attachments_on_a_message=1
attachments_zero_byte=3
attachments_with_content_id=0
attachments_method_by_value=2
attachments_method_embedded_message=1
attachments_method_ole=1
attachments_method_other=0
embedded_messages_opened=0
embedded_message_open_errors=0
```

Internal consistency: attachment-method buckets sum to `total_attachments`
(2+1+1=4); the recipient split (9 To, 1 CC, 1 BCC, max 2 on any message) is
consistent with two separate 2-recipient messages. No arithmetic anomalies.

Three findings need investigation before this data can be trusted:

1. **RESOLVED (2026-09-07).** `bodies_html=0` universally was not a
   fixture problem or an environment default — it was a real gap in this
   diagnostic's detection logic. Direct byte-level inspection of a
   deliberately-HTML fixture file (`This is an example HTML message.msg`,
   confirmed genuine HTML via its view-source: real `<html>`/`<body>`
   markup, Word-generated) proved the file has **no native
   `PidTagBodyHtml` (0x1013) property stream at all** — only `PidTagBody`
   (0x1000, plain) and `PidTagRtfCompressed` (0x1009, RTF) exist. Outlook
   had encapsulated the HTML inside the RTF body instead (MS-OXRTFEX), a
   legitimate and common MAPI storage strategy. This diagnostic originally
   checked only the native `.html` field and never used `msg_parser`'s own
   documented fallback for this exact case, `Outlook::html_from_rtf()`.
   Fixed: HTML detection now checks native `.html` first, then falls back
   to `html_from_rtf()`, and the two paths are tracked as distinct counters
   (`bodies_html_native`, `bodies_html_via_rtf`) rather than silently
   merged, consistent with this project's loss-transparency principle —
   `bodies_html` remains as an "either path" convenience total. This
   strongly suggests, though does not independently confirm for each file,
   that all prior 28 fixture messages share this same RTF-encapsulated-HTML
   storage pattern rather than being genuinely RTF-only as first assumed.
2. **`attachments_zero_byte=3` of 4 total attachments.** Two of these are
   plausibly explained by `payload_bytes` not being the right field to
   check for embedded-message/OLE attachment types (methods `5` and `6`),
   whose real content lives elsewhere (`as_message()`, or a different
   structure for OLE) — expected to read as empty via this field, not a
   sign of a genuine zero-byte file. The third, among the two `by_value`
   (regular file) attachments, is unexplained and needs a direct look at
   that file's actual size.
3. **The most significant finding: `embedded_messages_opened=0` and
   `embedded_message_open_errors=0`, despite `attachments_method_embedded_message=1`.**
   `msg_parser` classified one attachment as an embedded message via
   `attach_method`, but `Attachment::as_message()` returned `None` for it
   — not `Some(Ok(...))`, not `Some(Err(...))`. This is the specific
   capability that motivated choosing `msg_parser` over alternatives (see
   the 2026-09-07 comparative analysis), and this first real data point is
   a negative result for it. One file on one creation path is not enough
   to conclude the capability doesn't work, but it needs to be chased down
   — not glossed over — before `msg_parser`'s suitability is treated as
   settled.



## What remains unproven

The build and test suite passing, and this first run completing without a
crash, establish that the code compiles and doesn't fail outright — they
do not establish that:
- `msg_parser`'s field names/types match what this code assumes with full
  confidence (derived from the crate's README and docs.rs, not a full
  local source dump the way the P4b breakthrough was — lower confidence
  than that work carried, and this run's anomalies may be evidence of
  exactly that gap);
- embedded-message opening actually works against real data (this run's
  one data point is a `None`, not a success — see finding 3 above);
- body-type detection is measuring what it's intended to measure, given
  the uniform plain+RTF/zero-HTML result across all 9 files (finding 1);
- the zero-byte attachment count reflects genuine zero-byte files rather
  than an artifact of which field is being checked for which attachment
  type (finding 2);
- any of these counts are correct in the sense of matching the fixture's
  actual intended composition.

See project correspondence for the specific follow-up needed to resolve
the three open findings above.
