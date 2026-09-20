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
  ignored.

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
cargo test          # 28 passed, 0 failed
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

The 62 unrecognized entries are explicitly counted, rather than hidden. Their
structural scope and standardized naming pattern require a separate,
privacy-safe diagnostic before the classifier is extended. The signed gap
also makes a future overcount visible rather than masking it.

## Current scope

The verified implementation currently establishes that:

- `cfb` v0.14 can open a real `.msg` file;
- `cfb::walk()` can enumerate its CFB entries;
- principal MS-OXMSG entry naming conventions can be classified;
- variable-property stream names can yield MAPI property IDs;
- properties-stream presence and size can be observed;
- recipient and named-property storages can be identified; and
- unknown/unrecognized entries are explicitly counted.

It does **not** yet establish:

- complete MS-OXMSG structural accounting across the fixture corpus;
- fixed-property decoding from `__properties_version1.0`;
- variable-property value decoding;
- named-property mapping resolution;
- attachment-property decoding;
- embedded-message decoding; or
- production replacement of `msg_parser`.

Those remain subsequent milestones requiring further real-data runs,
implementation, and explicit tests.
