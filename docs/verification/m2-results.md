# M2 MSG Ingestion Spike

## Status

**Compiles and runs cleanly on Windows (21/21 tests pass) against real fixture sets on both a `.pst` and a `.msg` basis.** Body-type detection (native HTML, HTML-via-RTF-encapsulation, plain, RTF) is corrected and confirmed against real data as of 2026-09-14. Recipient/attachment classification and the zero-byte/method-conditioning fix are confirmed correct against real data. Embedded-message *opening* (not classification, which works) was investigated across two independent real attempts and found not achievable in practice — see "M2c" below; this is now an accepted, documented ceiling, not an open question.

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

1. **PARTIALLY RESOLVED, then corrected further (2026-09-07, then
   2026-09-13).** `bodies_html=0` universally was not a fixture problem or
   an environment default — it was a real gap in this diagnostic's
   detection logic. Direct byte-level inspection of a deliberately-HTML
   fixture file (`This is an example HTML message.msg`, confirmed genuine
   HTML via its view-source: real `<html>`/`<body>` markup, Word-generated)
   proved the file has **no native `PidTagBodyHtml` (0x1013) property
   stream at all** — only `PidTagBody` (0x1000, plain) and
   `PidTagRtfCompressed` (0x1009, RTF) exist. Outlook had encapsulated the
   HTML inside the RTF body instead (MS-OXRTFEX), a legitimate and common
   MAPI storage strategy. The 2026-09-07 fix added a fallback to
   `Outlook::html_from_rtf()`, treating its mere non-emptiness as "HTML was
   found." **That fallback was itself wrong, corrected 2026-09-13** — see
   "CORRECTED: fromhtml detection was not actually spec-gated" below.
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



## Critical review and fixes (2026-09-13)

Same review pass as the PST side (see `m1-results.md` for the full
writeup); findings specific to or mirrored on the MSG side:

- **`attachments_zero_byte` conflation, fixed identically to the PST
  side.** `payload_bytes.len() == 0` was counted regardless of
  `attach_method`. Embedded-message and OLE attachments don't populate
  `payload_bytes` the way by-value attachments do, so a zero reading there
  doesn't mean "empty file." Fixed: only `by_value` attachments with a
  zero-length payload count toward `attachments_zero_byte`; everything
  else goes into `attachments_zero_size_other_method`. This directly
  affects the still-open "mystery zero-byte attachment" investigation from
  2026-09-07 — re-running against the fixture folder should now show
  whether the one unexplained zero-byte hit among the `by_value`
  attachments is still there once the embedded-message and OLE
  attachments are correctly excluded from that counter.
- **Non-recursive directory scan now reports what it skips.** `tsp`
  prints `subdirectories_skipped=N` so the scope of a directory scan is
  visible in the output itself, not just in source comments.
- **New, MSG-specific structural limitation identified (not a bug, not
  fixed): no ORIG-recipient equivalent.** The PST side explicitly detects
  and buckets `PidTagRecipientType`'s rare "ORIG" value (0). `msg_parser`'s
  `Outlook` struct exposes only `to`/`cc`/`bcc` — there is no way to detect
  an ORIG-classified recipient on the MSG side through this crate's public
  API. Accepted as a known asymmetry between the two format adapters;
  not scheduled for a fix given how rare this recipient type is and the
  absence of any fixture evidence of one to test against.
- **`bodies_plain` interpretation caveat** — identical to the PST side,
  see `m1-results.md`.

## CORRECTED: fromhtml detection was not actually spec-gated (2026-09-13)

The 2026-09-07 fix trusted `Outlook::html_from_rtf()`'s mere non-emptiness
as proof of MS-OXRTFEX HTML encapsulation. **This was wrong**, discovered
while independently verifying the PST-side fix (see `m1-results.md`,
"CONFIRMED: PST-side HTML-in-RTF blind spot" and the PST-side fix that
followed it):

- Testing the PST-side fix against the live `tsp-tester.pst` produced an
  unexpected result: its one RTF-only message showed no FROMHTML marker
  at all (`bodies_html_via_rtf=0`), directly contradicting the earlier
  MSG-side finding that this same message (exported as `RTF_message.msg`)
  had `bodies_html_via_rtf=1`.
- Craig confirmed `RTF_message.msg` genuinely is the message in
  `tsp-tester.pst`, and used Outlook's own View Source feature on it. The
  resulting HTML carried an explicit `<!-- Converted from text/rtf
  format -->` comment and a `Generator: MS Exchange Server` tag — i.e.,
  Exchange generated that HTML at *render time* for the View Source
  display feature. That is an unrelated mechanism from MS-OXRTFEX
  encapsulation; its existence says nothing about whether `\fromhtml1` was
  ever present in the message's actual `PidTagRtfCompressed` property.
  (Contrast with the markup style of the genuinely-HTML-authored test
  file from 2026-09-07, which carried Word-specific CSS classes and
  `mso-`-prefixed styling comments — qualitatively different from this
  message's generic `<SPAN>`/`<FONT>` markup.)
- Conclusion: `html_from_rtf()` does not gate on the FROMHTML control word
  the way the specification requires for a real detection signal — it
  appears to perform RTF-to-HTML conversion unconditionally, the same way
  Exchange's View Source rendering does, regardless of whether the
  content was ever really HTML.

**Fixed**: the MSG-side diagnostic no longer calls `html_from_rtf()` at
all. It now uses `Outlook::rtf_decompressed()` to get the raw decompressed
RTF bytes and checks them directly for the literal `\fromhtml1` control
word — the exact same check, via a single shared function
(`rtf_bytes_contain_fromhtml`), that the PST side uses. Both diagnostics
now apply identically strict, specification-correct detection instead of
two different signals that could (and did) silently disagree. A new
`rtf_decompression_errors` counter was added to `MsgTotals`, mirroring the
PST side, for the case where `rtf_compressed` is present but
`rtf_decompressed()` returns `None`.

This means the earlier "27 of 29 messages via RTF" figure (recorded
2026-09-07) was suspected of being an overcount, and needed to be
re-measured with this corrected code before being trusted.

### RESOLVED (2026-09-14): re-measured at 27/29 again — and that's the correct figure, not a repeat of the bug

Re-running the corrected code against the same 29-file fixture produced
`bodies_html_via_rtf=27` again — numerically unchanged from the
pre-correction figure. That coincidence needed to be checked rather than
assumed either way (bug still present, or a genuine re-confirmation).

Isolating `RTF_message.msg` (the one file with independent, non-`tsp`
ground truth — confirmed via Outlook's own View Source feature to be
genuinely RTF-authored) and running it alone through the corrected code
showed `bodies_html_via_rtf=1` — appearing to contradict the PST-side
diagnostic's `bodies_html_via_rtf=0` for what should be the same
underlying message, despite both sides now sharing the identical
`rtf_bytes_contain_fromhtml` check.

A size-only diagnostic (`rtf_decompressed_bytes_total`) resolved this: the
PST-native message's decompressed RTF is 10,778 bytes; the exported
`.msg`'s is 3,191 bytes — a 3.4× difference, far beyond any decompression
quirk. **The exported file genuinely contains different, regenerated RTF
content, not the original PST-stored bytes** — most likely because
Exchange was actively resynthesizing this specific message's RTF/HTML
representations (consistent with the earlier View Source evidence), and
the resynthesized version picked up a `\fromhtml1` marker the PST-native
version never had. Full writeup: `m1-results.md`, "RESOLVED (2026-09-14):
the PST/MSG disagreement was a cross-export artifact, not a bug in either
fix."

**Conclusion: the corrected MSG-side detection is working correctly.**
`RTF_message.msg` was never a reliable stand-in for the PST-native
message's true content once exported — that's a limit of the
export-as-verification-proxy technique, not evidence against the fix.
The `27/29` figure reflects genuine `.msg` files, not PST exports subject
to this resynthesis behavior, and stands as the corrected, trustworthy
measurement.



## M2c: embedded-message opening investigated and found not achievable in practice (2026-09-14)

Mirroring P4c on the PST side (`outlook-pst` could not open embedded
messages through its public API), the same underlying capability was
investigated on the MSG side and reached the same practical outcome,
though for a different, less fully-understood reason.

`msg_parser` explicitly documents this as supported: attachments with
`attach_method == 5` are nested `.msg` files, and `Attachment::as_message()`
is documented to parse them recursively. `tsp`'s classification of
`attach_method` is confirmed correct — `attachments_method_embedded_message`
counts these attachments accurately across two independent fixture rounds.
But actually opening one has failed on every real attempt so far:

- **First attempt (2026-09-07):** one embedded-message attachment,
  `embedded_messages_opened=0`, `embedded_message_open_errors=0` — `None`,
  not an error.
- **Second attempt (2026-09-14), built via a different construction
  method specifically to rule out a fluke of the first:** a fixture round
  adding two more embedded-message attachments (three total across the
  enhanced `tsp-tester.pst`, two isolated in a dedicated
  `msgs\embedded` test folder) — same result:
  `embedded_messages_opened=0`, `embedded_message_open_errors=0` again.

Two independent, differently-constructed real attachments producing an
identical silent `None` — not an explicit `Err` either time — is no
longer treated as a fluke of one construction method. Something in
`as_message()`'s internal gating, separate from the `attach_method` check
(which demonstrably works), is not recognizing these as parseable.

**The exact root cause was not found.** `msg_parser`'s actual
`Attachment::as_message()` implementation was searched for across three
separate research attempts (varying search terms, direct GitHub source
lookups) and did not surface — a real limit of this investigation, stated
plainly rather than papered over with a plausible-sounding guess.

**Decision: stop here, do not invest further fixture-construction effort
chasing this.** Two consistent negative results with no explicit error is
treated as sufficient evidence of a real, practical limitation, mirroring
the evidentiary bar P4c was closed at. Two possible parallel, low-cost
next steps — filing an issue with the `msg_parser` maintainer, or
attempting to read the crate's actual source directly (e.g. from Craig's
local Cargo registry cache, the way the P4b column-read API was finally
confirmed after docs.rs research alone stalled) — are both cheap enough to
revisit later, but neither is being pursued as of this writing.

**A strategic point this raises, not just a technical one:** opening
embedded messages was one of the stronger points in favor of choosing
`msg_parser` over alternatives (see the 2026-09-07 comparative analysis)
— specifically, that it "already solves what P4c couldn't." That
specific differentiator has not held up against real data. `msg_parser`
still has real, demonstrated advantages over the alternative considered
(byte-level attachment content, named-property resolution, active
maintenance), but this one is documentation, not demonstrated behavior,
at least against real Outlook-constructed embeds tried so far. Worth
carrying forward accurately into the still-deferred "custom parser vs.
`msg_parser` in production" decision at M3, rather than relying on the
original comparison's now-partially-outdated reasoning.

`tsp` can *detect and count* embedded-message attachments on the MSG side
(confirmed correct, twice); it cannot open them — the same practical
ceiling as the PST side, for what now appears to be a different
underlying reason on each side (an unreachable public API on the PST
side; an internal gating condition of unknown cause on the MSG side).

## What remains unproven

The build and test suite passing, and this first run completing without a
crash, establish that the code compiles and doesn't fail outright — they
do not establish that:
- `msg_parser`'s field names/types match what this code assumes with full
  confidence (derived from the crate's README and docs.rs, not a full
  local source dump the way the P4b breakthrough was — lower confidence
  than that work carried, and this run's anomalies may be evidence of
  exactly that gap);
- embedded-message opening actually working against real data — investigated
  (M2c) across two independent attempts and found not to work in practice,
  for a root cause not identified despite three research attempts; `tsp`
  correctly detects and counts these attachments but cannot open them,
  mirroring P4c's PST-side ceiling;
- body-type detection is measuring what it's intended to measure — the
  original uniform plain+RTF/zero-HTML result (finding 1) revealed a real
  detection bug (`html_from_rtf()` not being spec-gated), corrected
  2026-09-13, re-run and confirmed 2026-09-14 (27/29 via RTF, cross-checked
  against a genuine size discrepancy explaining the one file that briefly
  looked like a contradiction — see the "RESOLVED (2026-09-14)" section
  above);
- the zero-byte attachment fix — compiled, run, and confirmed correct
  2026-09-14: the method-conditioned split (`attachments_zero_byte` vs.
  `attachments_zero_size_other_method`) behaves as designed against real
  data, including a newly-added embedded-message/OLE-attachment set where
  `attachments_zero_size_other_method` exactly matched the sum of those
  two methods' counts;
- any of these counts are correct in the sense of matching the fixture's
  actual intended composition.

See project correspondence for the specific follow-up needed to resolve
the remaining open findings above (embedded-message opening still
returning `None`, and body-type detection's interpretation, both largely
addressed but pending final confirmation).
