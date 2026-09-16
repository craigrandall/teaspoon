# M1 PST Feasibility Spike

## Status

**Complete, within the limits of `outlook-pst` v1.2.0's public API — with
one confirmed correction to that completeness claim, dated 2026-09-13: see
"CONFIRMED: PST-side HTML-in-RTF blind spot" below.** P1 through P4b are
done and verified on Windows against a real PST fixture deliberately
enhanced to close 5 of 7 identified representativeness gaps.
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

## CONFIRMED: PST-side HTML-in-RTF blind spot (2026-09-13)

The hypothesis raised on 2026-09-07 (that `tsp`'s PST-side `bodies_html`
check has the identical blind spot the MSG-side fix corrected) is now
**confirmed, not merely suspected.**

The message believed to be the PST's "RTF-only" fixture item was exported
via Outlook's Save As → `.msg` and independently verified two ways:

1. `tsp`'s own (already-fixed) MSG diagnostic reported
   `bodies_html_native=0`, `bodies_html_via_rtf=1` — HTML recovered only
   via `msg_parser`'s RTF-decoding fallback, not a native property.
2. Direct byte-level enumeration of the exported file's raw OLE/CFB
   property streams independently confirmed: `PidTagBody` (0x1000)
   present, `PidTagRtfCompressed` (0x1009) present, `PidTagBodyHtml`
   (0x1013) **absent**. Two independent methods agree completely.

Since PST and MSG serialize the identical MAPI/MS-OXPROPS property model
in different containers, and Outlook's export preserves rather than
manufactures properties, there is no reason to believe the original PST
message's property set differs from what this export shows.

**Concrete implication: `bodies_html=55/57` from the P4a evidence above is
very likely an undercount.** At least one message classified as
"RTF-only" is actually HTML-authored content that `outlook-pst`'s
diagnostic cannot see, because it only checks native `PidTagBodyHtml`
presence — the exact same single-field check that was wrong on the MSG
side before the 2026-09-07 fix.

**Fixing this is a larger lift than the MSG-side fix was**, because
`outlook-pst` has no `html_from_rtf()`-equivalent convenience method the
way `msg_parser` does. It would require: (1) extracting
`PidTagRtfCompressed`'s raw binary value (currently `tsp` only checks its
*presence*, never reads the value), (2) decompressing it per MS-OXRTFCP,
and (3) extracting the encapsulated HTML per MS-OXRTFEX. Candidate
crates identified for steps (2) and (3), not yet adopted or verified
against this project's fixtures:

- `compressed-rtf` (MS-OXRTFCP decompression) — maintained by the same
  author as `outlook-pst` (`wravery`/Bill Avery), which is a meaningful
  trust signal given that family relationship; exposes a simple
  `decompress_rtf(data: &[u8]) -> Result<String>`.
- A second crate for RTF→text/HTML extraction that correctly handles
  Outlook's `\fromhtml` encapsulation (skipping `\*\htmltag` destinations)
  would still be needed; one candidate (`rtf-parser`) was seen used for
  exactly this purpose in an unrelated third-party project, but has not
  been independently researched or vetted for this project.

**Decision made and implemented (2026-09-13): fixed, not merely
documented.** `tsp` now checks `PidTagRtfCompressed` for MS-OXRTFEX
encapsulation whenever native `PidTagBodyHtml` is absent, mirroring the
MSG-side fix exactly, and its output is now symmetric between the two
adapters: `bodies_html_native`, `bodies_html_via_rtf`, and a combined
`bodies_html`, plus `rtf_decompression_errors` for the case where the
property is present but doesn't decompress cleanly. See "PST-side
HTML-in-RTF fix, implementation details" below for what changed and why
a full RTF-to-HTML extractor was deliberately *not* built.

### PST-side HTML-in-RTF fix, implementation details (2026-09-13)

Only step 2 of the three steps outlined above (decompression) was
implemented; step 1 (extracting the raw value, not just checking
presence) was needed to feed it; **step 3 (extracting the actual HTML
text) was deliberately not built**, because it isn't needed. Per
MS-OXRTFEX's own "Recognizing RTF Containing Encapsulation" section, a
de-encapsulating reader that finds the FROMHTML control word
(`\fromhtml1`) "SHOULD conclude the RTF document contains encapsulated
HTML and stop further inspection" — presence of that one control word is
the specification-sanctioned detection signal, not a heuristic
substitute for real extraction. Since `tsp` is a privacy-safe presence/
count diagnostic that has never printed body content on either format
side, checking for this one marker is sufficient; nothing was gained by
also adopting a full RTF-to-HTML converter, so `rtf-parser` (flagged as
an unvetted candidate above) was not pursued.

`compressed-rtf` (MS-OXRTFCP decompression) was adopted, with its magic
numbers and dictionary cross-checked against `msg_parser`'s own
independent from-scratch implementation of the same algorithm — both
agree exactly, which is meaningful corroboration from two unrelated
authors' implementations of the same Microsoft spec.

One real defensive finding from reading `compressed-rtf`'s actual source
rather than trusting its signature: `decompress_rtf` indexes into the
first 16 bytes of its input unconditionally as part of its own header
read, and **panics** rather than returning an error if given fewer bytes.
`tsp` now guards this explicitly (any `PidTagRtfCompressed` value under
16 bytes is treated as a decompression failure before ever calling the
crate function), so a truncated or corrupt property cannot crash `tsp`.
This was not something the crate's public API signature (`fn
decompress_rtf(data: &[u8]) -> Result<String>`) would have revealed —
only reading the actual implementation did.

**Verification status, and a second, more important finding it led to
(2026-09-13):** the first Windows run against `tsp-tester.pst` after this
fix returned `bodies_html_native=55`, `bodies_html_via_rtf=0` — unchanged
from before the fix, and the opposite of what the "CONFIRMED" evidence
above predicted (`bodies_html_via_rtf=1` for the one RTF-only message).
`rtf_decompression_errors=0` confirmed this was a clean negative result,
not a crash or failure, which made the contradiction worth chasing rather
than dismissing.

**Resolution: this PST-side result was correct, and the earlier MSG-side
result was not.** Craig confirmed `RTF_message.msg` genuinely is the PST's
RTF-only message, then used Outlook's own View Source feature on it. The
resulting HTML carried an explicit `<!-- Converted from text/rtf
format -->` comment and a `Generator: MS Exchange Server` tag — i.e.,
Exchange generated that HTML at *render time* for the View Source display
feature, an unrelated mechanism from MS-OXRTFEX encapsulation that says
nothing about whether `\fromhtml1` was ever in the message's actual RTF.
This message is genuinely RTF-authored. The earlier MSG-side finding
(`bodies_html_via_rtf=1` via `msg_parser::Outlook::html_from_rtf()`) was a
false positive: that method turned out not to gate on the FROMHTML
control word at all. See `m2-results.md`, "CORRECTED: fromhtml detection
was not actually spec-gated," for the fix this led to on the MSG side —
both diagnostics now share one function (`rtf_bytes_contain_fromhtml`)
checking the literal control word against decompressed bytes, rather than
each trusting a different crate's higher-level convenience method.

Unit tests cover the pure branching logic (absent property, non-binary
property, too-short buffer, and a structurally-invalid-but-correctly-sized
buffer all correctly avoid being misread as "no encapsulated HTML found"),
plus the shared marker-matching function directly. The genuine
`\fromhtml1`-present success path has now been indirectly exercised via
real data on the MSG side (once corrected) but not yet reconfirmed on the
PST side with a message actually known to contain the marker — the one
PST message tested turned out to be a true negative, not a positive.

### RESOLVED (2026-09-14): the PST/MSG disagreement was a cross-export artifact, not a bug in either fix

After the correction above, re-testing surfaced one more apparent
contradiction: the PST-side diagnostic, run directly against the live
`tsp-tester.pst`, reported `bodies_html_via_rtf=0` for its one RTF-only
message — but the MSG-side diagnostic, run against that same message
exported as `RTF_message.msg`, reported `bodies_html_via_rtf=1`. Since
both sides now call the identical `rtf_bytes_contain_fromhtml` on
decompressed bytes, the disagreement had to be in what bytes each side
was actually checking, not the check itself.

A privacy-safe size-only diagnostic (`rtf_decompressed_bytes_total`, sum
of decompressed RTF byte lengths, never content) was added to both sides
to test this without needing the file's content again. Result:

- PST-native decompressed RTF: **10,778 bytes**
- Exported `.msg`'s decompressed RTF: **3,191 bytes** (3.4× smaller)

This is not a rounding difference or a minor decompression-implementation
quirk between `compressed-rtf` and `msg_parser` — it is conclusive
evidence that **the exported `.msg` file genuinely contains different,
substantially smaller RTF content than the live PST message does.**
Combined with the earlier View Source evidence (`<!-- Converted from
text/rtf format -->`, `Generator: MS Exchange Server`), the most likely
explanation is that Outlook's Save As → `.msg` export triggered Exchange
to resynthesize a simplified RTF representation for this specific
actively-synced message, rather than copying the stored
`PidTagRtfCompressed` bytes verbatim — and that resynthesized version
picked up a `\fromhtml1` marker the original never had.

**Conclusion: both fixes are correct.** The PST-side result
(`bodies_html_via_rtf=0`) reflects the message's true, stored content.
The MSG-side result for this one *exported* file reflects different,
regenerated content — not a flaw in the MSG-side detection logic itself.
This also means the aggregate MSG-side finding (27 of 29 `.msg` files
showing `bodies_html_via_rtf=1`) should **not** be discounted on the
strength of this one file: those 29 files are genuine `.msg` files, not
PST-exports subject to this specific resynthesis behavior, so there is no
similar reason to doubt them without separate evidence.

**Methodological takeaway, not a code change:** exporting a PST message
to `.msg` via Save As is not guaranteed to reproduce byte-identical
property content, at least for messages Exchange is actively
RTF/HTML-syncing. This is a real limit on using cross-format export as a
verification technique going forward, worth remembering the next time a
PST-side and MSG-side result are compared this way.

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

## Critical review and fixes (2026-09-13)

Following the HTML-detection fix, the codebase was reviewed specifically
for other instances of the same failure pattern: a single-field check
whose absence was silently treated as ground truth, when the underlying
spec or dependency has another storage path or edge case for the same
concept. Two real issues were found and fixed on the PST side (the same
fixes were applied symmetrically on the MSG side; see `m2-results.md`):

- **`attachments_zero_byte` was conflating two different things.**
  `PidTagAttachSize == 0` was counted as "empty attachment" regardless of
  the attachment's method. But for embedded-message and OLE attachments,
  the real content lives outside this property entirely, so a zero
  reading there is structurally expected, not evidence of a genuinely
  empty file — the exact case Craig's own empirical research (Gmail/Outlook
  zero-byte experiments) was about. Fixed: zero-size readings are now only
  counted in `attachments_zero_byte` when the attachment's method is
  `by_value`; every other method's zero-size reading goes into a new,
  separately-tracked `attachments_zero_size_other_method` counter.
- **The non-recursive `.msg` directory scan produced no visible signal
  that it was non-recursive.** If a fixture folder had `.msg` files
  nested in subfolders, `tsp` would silently undercount with nothing in
  the printed output indicating anything was skipped. Fixed: `tsp` now
  reports `subdirectories_skipped=N` in its MSG diagnostic output.

Two further items were identified as interpretation caveats rather than
code defects, and are now documented in code comments rather than fixed,
since there is nothing to fix — the data is accurate, only its
interpretation was previously assumed rather than stated:

- **`bodies_plain` reflects property presence, not authored format.**
  Outlook commonly populates a plain-text compatibility mirror alongside
  an HTML- or RTF-authored body regardless of composition intent. High
  `bodies_plain` counts should not be read as "many messages were
  plain-text-authored."
- **`read_i32_at`'s `PropertyValue::Integer32` type assumption has never
  been falsified** for `PidTagRecipientType`/`PidTagAttachMethod`/
  `PidTagAttachSize`, because every fixture message so far has happened to
  store these as that type. The failure mode if this assumption is ever
  wrong is safe (falls to an explicit "unknown" bucket), but the
  assumption itself remains untested against data that would break it.

One item was identified as a known, accepted structural limitation on the
MSG side specifically (no PST-side equivalent needed since `outlook-pst`
already supports it) — see `m2-results.md`.

## What remains unproven

No claim is made that teaspoon has proven:
- the 2026-09-13/14 RTF-encapsulated-HTML fix's positive-detection path
  (real `\fromhtml1` presence, correctly detected) against a real *PST*
  message specifically — the negative path is confirmed (correctly
  finding no marker in a genuinely RTF-authored PST message), and the
  positive path is confirmed on the *MSG* side (27 real `.msg` files),
  but no PST message in the current fixture is known to contain the
  marker to test the PST side's positive path directly;
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
- the 2026-09-13 zero-byte/attachment-method fix (`attachments_zero_byte`
  now conditioned on `by_value`, plus the new
  `attachments_zero_size_other_method` counter) — implemented, not yet
  compiled or run on Windows;
- `subdirectories_skipped` reporting — implemented, not yet run;
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
