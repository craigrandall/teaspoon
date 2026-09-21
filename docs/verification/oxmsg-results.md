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

### Known ambiguity to fix before decoding

Streams inside `__nameid_version1.0` (`00020102`, `00030102`, `00040102` and the
`0x1000`-range hash-bucket streams) are not MAPI properties. The unscoped
`property_id` totals mix them with real properties: unscoped `0x1000` is 33 =
29 message + 1 embedded-object + 3 named-property buckets, and `0x1009` is 54.
The scoped lines are correct; the unscoped ones are not usable for decoding.

## Current scope (replacement)

The verified implementation now establishes that:

- `cfb` v0.14 opens and enumerates every entry of all 29 fixtures;
- every entry is accounted for (`entry_accounting_gap_total=0`) across the
  corpus, with none unrecognized;
- MS-OXMSG entry naming, including multiple-valued value streams, is classified;
- embedded-object storages are distinguished as message-shaped or custom, and
  custom-storage contents are reported as opaque payload;
- entries are attributed to message, recipient, attachment, embedded-object and
  named-property scopes.

It still does **not** establish: fixed-property decoding, variable-property
value decoding, named-property mapping resolution, attachment-property
decoding, embedded-message decoding, or production replacement of `msg_parser`.
