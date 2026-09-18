# Custom MS-OXMSG Parser Groundwork (experimental, opt-in via `--oxmsg`)

## Status

**Implemented (P1/P2-equivalent), pending Windows compilation and a real
`.msg` fixture run.** Not yet verified — no real-data evidence exists yet
for this path, unlike `msg_parser`'s adapter, which has two confirmed
rounds of real-fixture evidence behind it. Treat every number this
diagnostic produces as unconfirmed until a real run comes back.

## Why this exists

`msg_parser` has no raw/generic property-iteration equivalent to
`outlook-pst`'s `MessageProperties::get(id)`/`.iter()`. That is the one
structural inconsistency remaining between teaspoon's two format adapters,
and it is directly at odds with design principle 4 (unknown/unmapped
properties are preserved or reported, not silently discarded) and ADR:
loss-aware-normalized-representation. The PST side can tell you a property
existed but couldn't be read; the MSG side, via `msg_parser` alone, cannot
make that distinction for anything outside the specific fields the crate's
`Outlook` struct happens to expose.

This was raised as a live decision (not defaulted into) once the original
condition for deferring it — "wait until loss-accounting requirements for
MSG are concrete" — was actually met through real evidence: `msg_parser`'s
embedded-message-opening capability, one of the stronger original
arguments for choosing it, was shown not to work in practice (see M2c,
`docs/verification/m2-results.md`), while its core detection logic
(`html_from_rtf()`) had already been shown unreliable and replaced with
teaspoon's own spec-correct check. The decision: build a custom MS-OXMSG
parser incrementally, reusing a generic, well-established CFB reader for
the container-parsing layer rather than reimplementing that from scratch,
and writing only the Outlook-specific property-table interpretation
directly — mirroring the level of control `outlook-pst` already provides
on the PST side.

This is explicitly **not** a decision to replace `msg_parser` immediately.
Both adapters exist side by side; `--oxmsg` opts into this experimental
path for `.msg` input, and the default `msg_parser`-based diagnostic is
unchanged. ADR-0001 (format-independent domain model) means either can
be the production MSG adapter later without disturbing the rest of the
pipeline.

## Dependency

`cfb` v0.14 (crates.io, MIT) — a mature, general-purpose Rust reader for
MS-CFB (Compound File Binary) containers, the same generic container
format underlying legacy `.doc`/`.xls`/`.ppt`/`.msi` as well as `.msg`.
Chosen specifically so the container-parsing layer itself does not need
to be built from scratch — only the MS-OXMSG-specific naming convention
layered on top of it is teaspoon's own code. Not otherwise Outlook- or
mail-specific; it knows nothing about MAPI, properties, or message
semantics.

## What this diagnostic checks

For each `.msg` file, `cfb::open` opens the container and `walk()`
enumerates every entry (storage or stream) in it. Each entry's *name* is
classified against the well-known MS-OXMSG naming conventions — never its
content:

- `__properties_version1.0` — the single stream holding every
  *fixed-length* property, packed together. Its presence and byte length
  are reported; its packed contents are not yet decoded (that is later
  work, not part of this P1/P2-equivalent stage).
- `__substg1.0_PPPPTTTT` — one stream per *variable-length* property
  (strings, binary, multi-valued), where `PPPP` is the 4-hex-digit MAPI
  property ID and `TTTT` is the 4-hex-digit property type. Reported as an
  aggregate count per property ID (a bounded, standard MAPI vocabulary,
  the same treatment already given to message-class names elsewhere in
  this codebase) — never the stream's content.
- `__attach_version1.0_#NNNNNNNN` / `__recip_version1.0_#NNNNNNNN` — the
  numbered sub-storages MS-OXMSG uses for each attachment/recipient.
  Counted, not opened or traversed.
- `__nameid_version1.0` — the storage holding named (non-standard)
  property mappings. Presence only.
- Anything else — counted as `unrecognized_entries_total` rather than
  silently ignored, consistent with the no-silent-loss principle. A
  nonzero count here means either an MS-OXMSG structure this parser
  doesn't yet know about, or something worth a closer look.

The `__substg1.0_PPPPTTTT` naming convention was not assumed from
documentation alone — it was independently confirmed by hand three times
earlier in this project via raw byte-level forensic inspection of real
`.msg` files (e.g. `__substg1.0_1000001F` = `PidTagBody`, `PT_UNICODE`,
confirmed while investigating the HTML-in-RTF blind spot). The unit tests
below use that exact same real example rather than a made-up one.

## Verification status

Unit tests cover the pure name-classification logic
(`classify_oxmsg_entry`) directly — every known naming convention, the
previously-confirmed real property-stream name, and malformed input
(non-hex characters, wrong length) correctly falling through to
`Unrecognized` rather than panicking. This function takes a plain `&str`
rather than a `cfb::Entry` directly, specifically because `Entry` has no
public constructor — keeping the classification logic string-based is
what makes it unit-testable at all without a real CFB file on disk.

**What is not yet verified**: whether this compiles against the real
`cfb` v0.14 API as documented (confirmed via docs.rs and source across
multiple crate versions, but never compiled — the same calibration note
every dependency in this project gets on first use); whether `cfb::open`
successfully opens a real `.msg` file; whether the property/storage counts
this produces are plausible against a fixture whose structure is already
known from `msg_parser`'s output (e.g. a fixture with confirmed
recipients/attachments should show consistent `recipient_storages_total`/
`attachment_storages_total` counts).

## Suggested first real run

Point `--oxmsg` at a `.msg` file (or folder) already characterized by the
`msg_parser`-based diagnostic, and sanity-check the two against each
other — for example, a file `msg_parser` reports as having 2 attachments
and 1 recipient should show `attachment_storages_total=2`,
`recipient_storages_total=1` here. Cross-checking both adapters against
the same real file, the same technique that caught the `html_from_rtf()`
bug, is the natural first verification step for this one too.

## What remains unproven

No claim is made that this diagnostic has proven:
- it compiles against the real `cfb` v0.14 API;
- it can open a real `.msg` file at all;
- any of its structural counts are correct against real data;
- the `__properties_version1.0` fixed-length property packing format —
  not yet decoded, only its presence and size reported;
- any property *value* — this stage is presence/name/size only, by
  design, mirroring exactly how M1's P1/P2 started before P4a/P4b added
  actual classification;
- named-property resolution — `__nameid_version1.0`'s internal mapping
  structure is not yet decoded, only its presence reported;
- that this path will ultimately replace `msg_parser` in production — an
  open decision, not resolved here.

Those require further real-data runs, further implementation, and
explicit tests — the same discipline every other part of this project has
been held to.
