# Custom MS-OXMSG Parser Groundwork (experimental, opt-in via `--oxmsg`)

## Status

**Compiled, quality-gated, and run successfully on Windows against a real
`.msg` fixture.** This experimental path now has first real-file structural
evidence. It remains a name/size/count diagnostic only: it does not yet
decode property values or establish complete MS-OXMSG support.

The first fixture run initially appeared to expose a one-entry accounting
discrepancy. A subsequent corpus run resolved that appearance: the comparison
mistook a per-file property-stream presence counter for an entry counter.
Every enumerated entry is now accounted for explicitly.

## Why this exists

`msg_parser` has no raw/generic property-iteration equivalent to
`outlook-pst`'s `MessageProperties::get(id)`/`.iter()`. That is the one
structural inconsistency remaining between teaspoon's two format adapters,
and it is directly at odds with design principle 4 (unknown/unmapped
properties are preserved or reported, not silently discarded) and ADR:
loss-aware-normalized-representation. The PST side can tell you a property
existed but could not be read; the MSG side, via `msg_parser` alone, cannot
make that distinction for anything outside the specific fields the crate's
`Outlook` struct happens to expose.

This was raised as a live decision once the original condition for deferring
it — wait until loss-accounting requirements for MSG are concrete — was met
through real evidence. `msg_parser`'s embedded-message-opening capability,
one of the stronger original arguments for choosing it, was shown not to work
in practice (see M2c, `docs/verification/m2-results.md`), while its core
detection logic (`html_from_rtf()`) had already been shown unreliable and
replaced with teaspoon's own spec-correct check.

This is explicitly **not** a decision to replace `msg_parser` immediately.
Both adapters exist side by side; `--oxmsg` opts into this experimental path
for `.msg` input, and the default `msg_parser`-based diagnostic is unchanged.

## Dependency

`cfb` v0.14 (crates.io, MIT) is the mature generic reader for MS-CFB
(Compound File Binary) containers. It handles the generic container layer;
teaspoon supplies only the MS-OXMSG-specific naming-convention layer. It
knows nothing about MAPI, properties, or message semantics.

## What this diagnostic checks

For each `.msg` file, `cfb::open` opens the container and `walk()` enumerates
every entry (storage or stream). Each entry name is classified against
well-known MS-OXMSG conventions — never by inspecting content:

- `__properties_version1.0` — the stream holding fixed-length properties.
  Per-file presence, every-entry count, and byte length are reported; packed
  contents are not decoded.
- `__substg1.0_PPPPTTTT` — variable-length property streams. `PPPP` is the
  four-hex-digit MAPI property ID and `TTTT` the four-hex-digit property type.
  Counts are reported by property ID, never by stream content.
- `__attach_version1.0_#NNNNNNNN` / `__recip_version1.0_#NNNNNNNN` — numbered
  attachment and recipient sub-storages. They are counted, not opened.
- `__nameid_version1.0` — named-property mapping storage. Its count and a
  compatibility-friendly presence result are reported.
- Anything else — counted as `unrecognized_entries_total`, never silently
  ignored. A privacy-safe breakdown records its CFB object kind, depth, and
  standardized name shape; neither entry names nor paths are emitted.

Recognized names are also checked against the CFB object type. A known stream
name represented by a storage (or a known storage name represented by a
stream) is reported as a type mismatch, rather than silently assumed to have
the expected semantics.

The `__substg1.0_PPPPTTTT` convention was independently confirmed earlier
through raw inspection of real `.msg` files (for example,
`__substg1.0_1000001F` is `PidTagBody`, `PT_UNICODE`).

## Verification status

### Quality gate

The repository quality gate completed successfully on Windows:

```text
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test          # 30 passed, 0 failed
cargo build --release
```

### Real-fixture verification

The experimental `--oxmsg` path then opened a real RTF `.msg` fixture. Its
privacy-safe structural result was:

```text
inventory=privacy_safe
input_kind=msg_oxmsg
files_scanned=1
subdirectories_skipped=0
open_errors=0
total_entries=60
has_properties_stream=1              # files with one or more such streams
properties_stream_entries_total=2    # root and recipient storage streams
properties_stream_bytes_total=1080
property_streams_total=55
attachment_storages_total=0
recipient_storages_total=1
has_named_property_storage=1
unrecognized_entries_total=0
```

The same run enumerated MAPI property IDs represented by variable-property
streams, including `0x1000`, `0x1009`, `0x300B`, and the `0x8000`+
named-property range. This is structural evidence only; no property values
were decoded.

### Entry accounting

The original 60-entry fixture was initially summarized as 59 categorized
entries because `has_properties_stream=1` was treated as an entry count. That
field is deliberately a per-file presence counter: it reports whether a file
has at least one `__properties_version1.0` stream. The recipient storage in
that fixture has its own property stream, producing two property-stream
entries in total.

The accounting diagnostic makes the distinction explicit with:

- `root_entries_total`;
- `properties_stream_entries_total`;
- `recognized_entries_total`;
- `unrecognized_entries_total`; and
- signed `entry_accounting_gap_total`.

The 29-file Windows corpus confirmed complete accounting:

```text
total_entries=2647
root_entries_total=29
properties_stream_entries_total=97
recognized_entries_total=2585
unrecognized_entries_total=62
entry_accounting_gap_total=0
```

The 62 unrecognized entries are explicitly counted, rather than hidden. The
diagnostic groups them by CFB object kind, depth, and name shape, and reports
recognized-name/object-type mismatches. This establishes structural scope
without exposing entry names, paths, or contents; only then can a classifier
extension be considered. The signed gap also makes a future overcount visible
rather than masking it.

### Unrecognized-entry diagnostic

The 29-file Windows corpus produced this privacy-safe breakdown:

```text
total_entries=2647
root_entries_total=29
recognized_entries_total=2641
opaque_payload_entries_total=6
unrecognized_entries_total=0
entry_accounting_gap_total=0
embedded_object_storages_total=2
embedded_object_storages_message_shaped_total=1
embedded_object_storages_custom_total=1
embedded_object_storage shape=custom clsid=f4754c9b-64f5-4b40-8af4-679732ac0607 count=1
embedded_object_storage shape=message_shaped clsid=00000000-0000-0000-0000-000000000000 count=1
opaque_payload_entry kind=stream depth_below_payload=1 count=6
```

### What was resolved, and how

Earlier corpus runs left 62 unrecognized entries plus two name/type mismatches.
They resolved as follows:

| Earlier finding | Count | Resolution |
| --- | --- | --- |
| depth-1 `malformed_property_stream_name` streams | 56 | Standard multiple-valued value-stream names (`__substg1.0_PPPPTTTT-NNNNNNNN`), recognized in v0.1.9. Corpus shows `indexed_property_streams_total=56`. |
| `stream_name_is_storage` mismatches | 2 | The two `__substg1.0_3701000D` embedded-object storages, now classified as storages. No mismatch lines remain. |
| depth-3 `other_name` streams | 6 | Direct children of the single **custom** embedded-object storage. Counted as `opaque_payload`. |

### The six streams

- `opaque_payload_entry kind=stream depth_below_payload=1 count=6`: all six are
  direct children of the one custom `__substg1.0_3701000D` storage. The corpus
  has exactly one such storage, so no other placement is possible.
- That storage has no `__properties_version1.0`, so it is not message-shaped.
  Its CLSID is `f4754c9b-64f5-4b40-8af4-679732ac0607`, the class identifier
  registered for `Word.Document` (Word 2007-era document objects).
- MS-OXMSG "Custom Attachment Storage" leaves the content format of such a
  storage to the application that produced it. These six streams are therefore
  Word's own OLE object streams, outside MS-OXMSG naming.

**Classification decision.** The six are counted in a separate
`opaque_payload` category, not folded into `recognized_entries_total`.
`recognized` means "an MS-OXMSG name we understand"; `opaque_payload` means
"structurally accounted for, contents not interpreted". That the corpus reaches
`unrecognized_entries_total=0` follows from this structural rule, and the
`unrecognized` category remains in the diagnostic for any future file where an
entry sits outside every known container.

### The two embedded-object storages

| Storage | Shape | CLSID | Cross-check |
| --- | --- | --- | --- |
| 1 | message-shaped (directly contains `__properties_version1.0`) | nil | `msg_parser` reports `attachments_method_embedded_message=1` |
| 2 | custom (no `__properties_version1.0`) | Word.Document | `msg_parser` reports `attachments_method_ole=1` |

The counts agree. This is a corpus-level correspondence, not a per-attachment
binding: the `--oxmsg` path does not yet decode `PidTagAttachMethod`.
`msg_parser` reports `embedded_messages_opened=0` despite the one
embedded-message attachment, consistent with the M2c finding.

### Cross-checks against `msg_parser`

| Quantity | `--oxmsg` | `msg_parser` | Difference |
| --- | --- | --- | --- |
| recipient storages vs `to+cc+bcc` | 37 | 36 | 1 |
| attachment storages vs `total_attachments` | 30 | 29 | 1 |

Both differences fit the embedded message's own contents (one recipient, one
attachment) being visible to `--oxmsg` but not to `msg_parser`. The recipient
difference cannot yet be told apart from a recipient that `msg_parser` cannot
represent (its `ORIG` limitation, see `record_msg_recipients`). Neither
difference is explained until properties are decoded.

### Internal consistency

- Categories sum to the total: 29 root + 97 properties streams + 2417 property
  streams + 30 attachment + 37 recipient + 29 named-property + 2 embedded-object
  = 2641, plus 6 opaque = 2647.
- Properties streams by scope: 29 message + 37 recipient + 30 attachment + 1
  embedded-object = 97. Each recipient and attachment storage has exactly one.
- Named-property storages: 29, one per file, none inside the embedded message,
  consistent with embedded messages sharing the top-level name mapping.
- `properties_stream_bytes_total=53504`. With headers of 32 bytes (message),
  24 (embedded message) and 8 (recipient, attachment), the remainder is
  52016 bytes = 3251 x 16, so the sizes are consistent with 16-byte fixed-length
  property entries. This is a consistency check on the total, not decoding.

### Named-property-storage streams excluded from unscoped totals

Streams inside `__nameid_version1.0` (`00020102`, `00030102`, `00040102` and the
`0x1000`-range hash-bucket streams) are not MAPI properties, and are now
excluded from the unscoped `property_id` map (the scoped map already kept
them separate). The corpus confirms it: unscoped `0x1000=30` (29 message + 1
embedded-object), matching the sum of the scoped message and embedded-object
lines, with no `0x0002`/`0x0003`/`0x0004` lines in the unscoped output.

### Property entry decoding (first slice)

The 29-file corpus confirms clean decoding of every properties-stream entry
array:

```text
properties_entries_total=3251
properties_entries_fixed_inline_total=1221
properties_entries_variable_single_total=2019
properties_entries_variable_multivalued_total=11
properties_stream_too_short_for_header_total=0
properties_stream_trailing_bytes_total=0
properties_stream_unexpected_scope_total=0
properties_stream_read_errors=0
attach_data_object_size_sentinel_mismatches=0
attach_data_object_reserved_vs_embedded_object_storage_gap=0
attach_data_object_reserved_vs_custom_object_storage_gap=0
```

The three shape totals sum to `properties_entries_total` exactly (checked
per scope against the `property_type` histogram, not just at the aggregate
level). The `attach_data_object` cross-check — the attachment's
`0x3701`/`PT_OBJECT` entry's Reserved field (0x01 embedded, 0x04 storage)
compared against the CFB-structural message-shaped/custom classification
from M2.x — agrees exactly: one entry reads `reserved=0x01`, one reads
`reserved=0x04`, matching the one message-shaped and one custom
embedded-object storage found earlier. Two independent signals, same
conclusion.

The property-type histogram also confirms `0x0048` (PT_CLSID, a 16-byte
GUID) is correctly classified as variable — GUIDs don't fit the entry's
8-byte inline value field even though MAPI treats the type as "fixed" in
the conceptual sense.

### Variable-value reading and named-property resolution: both verified

The 29-file corpus confirms variable-length value reading is clean:

```text
variable_value_stream_found_total=2017
variable_value_stream_missing_total=0
variable_value_size_mismatch_total=0
variable_value_odd_utf16_length_total=0
fixed_boolean_invalid_encoding_total=0
```

(2017, not 2019, is correct: it excludes the 2 `PT_OBJECT` entries already
accounted for via the embedded-object-storage path.)

Named-property resolution initially surfaced a real anomaly (298 of 436
occurrences resolving to an undefined GUID index, none as string-named).
Root cause: the Entry Stream's Index-and-Kind field was decoded backwards
on every axis -- which 16-bit half holds Property Index vs. GUID
Index/Kind, and within the GUID/Kind half, which bit is Kind. Found by
byte-level decoding of real fixture data (the same method that resolved
the RTF-in-HTML ambiguity), not by further inference from spec text or
crate source: file 0's GUID stream position 0 decodes to
`00062008-0000-0000-C000-000000000046` (PSETID_Common) exactly, and
applying the corrected formula to file 9's entries whose GUID resolves to
PSETID_Task yields LIDs `0x8101` and `0x8102` -- `PidLidTaskStatus` and
`PidLidPercentComplete`, MS-OXPROPS' own canonical names for exactly that
pair. Confirmed the same way across four files, sixteen entries, zero
exceptions on the Property Index cross-check (which independently
confirmed the OTHER 16-bit half's identity: it always equals the entry's
own array position).

The earlier "138 resolved" entries under the old code were not real
matches -- `guid_index` was being read from Property Index, so any entry
at array position 1 or 2 spuriously resolved as PS_MAPI/PS_PUBLIC_STRINGS
purely because those sentinel values coincide with common early array
positions.

**The fix is confirmed on the full 29-file corpus:**

```text
named_properties_seen_total=436
named_properties_guid_out_of_range_total=0
named_properties_index_mismatch_total=0
named_properties_string_kind_total=315
named_properties_numeric_kind_total=121
named_property_set set=PSETID_Common count=183
named_property_set set=PSETID_Task count=3
named_property_set set=custom count=250
```

Both gates the fix targeted read 0. Set-membership counts (183+3+250) and
kind counts (315+121) each sum to `named_properties_seen_total` exactly --
every occurrence lands in one set bucket and one kind bucket, nothing
double-counted or dropped. The large jump in string-kind properties (0 to
315) is expected, not a regression: the dominant `custom`-bucketed GUID is
very likely `PS_INTERNET_HEADERS` (confirmed present in file 0's GUID
stream during the investigation, not yet added to the well-known-set
table), whose named properties are string-named by construction (each is a
MIME header name). Named-property set resolution is verified.

### M3a — fixed-value interpretation

`decode_fixed_value` decodes every fixed-length entry's 8-byte value field
into its real type (`DecodedFixedValue`). Kept as a lossless, in-memory
intermediate representation only — nothing from it is printed by the
`--oxmsg` diagnostic, which stays exactly as content-free as every earlier
slice. Wired in as two structural checks: the existing boolean-encoding
validity check, and a new NaN/infinite check for `PT_FLOAT`/`PT_DOUBLE`/
`PT_APPTIME`. Both read 0 on the full corpus.

A `PT_SYSTIME` plausibility check (flagging implausible calendar dates)
was considered and deliberately not added — MS-OXOCAL's legitimate "no
end date" convention for recurring calendar items lands around the year
4500, so a naive range check would misfire on real, correct data.

Separately, adding `PS_INTERNET_HEADERS` (`{00020386-0000-0000-C000-000000000046}`)
to the well-known property-set table split the `custom` bucket exactly as
predicted: `custom` dropped from 250 to 143, with the other 107 now
correctly labeled `PS_INTERNET_HEADERS` — confirming with real data, not
just plausibility, that internet-header named properties (string-named by
construction) were the dominant contributor to what had looked like a
large "unknown" bucket.

## Current scope (replacement)

The verified implementation now establishes that:

- `cfb` v0.14 opens and enumerates every entry of all 29 fixtures;
- every entry is accounted for (`entry_accounting_gap_total=0`) across the
  corpus, with none unrecognized;
- MS-OXMSG entry naming, including multiple-valued value streams, is classified;
- embedded-object storages are distinguished as message-shaped or custom, and
  custom-storage contents are reported as opaque payload;
- entries are attributed to message, recipient, attachment, embedded-object and
  named-property scopes;
- every `__properties_version1.0` entry array is decoded (type, ID, flags,
  and variable-length size/reserved), with an independent property-level
  confirmation of the M2.x message-shaped/custom classification;
- variable-length value streams are located and structurally validated
  (declared size vs. actual length, UTF-16 byte-parity) without reading
  their content;
- named properties are resolved to their GUID set and numeric/string
  identity, verified against real fixture bytes byte-by-byte.

It still does **not** establish: fixed- or variable-property *value*
content (only structural shape and validity), embedded-message decoding,
or production replacement of `msg_parser`.
