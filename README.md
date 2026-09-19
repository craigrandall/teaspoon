# teaspoon (tsp)

This project implements standalone Rust CLI for deterministic, loss-aware mining of Outlook `.pst` and `.msg` 
files into a durable Markdown+metadata archive, without requiring Classic Outlook.

The shorthand for teaspoon (i.e. the name of this project) is tsp (i.e. the name of this project's tool), 
which is "pst" backwards. (ツ)

## Quick Start

```bash
# Build
cargo build --release

# Inventory a PST file (privacy-safe, no content exposed)
./target/release/tsp.exe my-archive.pst

# Inventory a directory of MSG files
./target/release/tsp.exe ./msgs/

# Use experimental CFB-based MS-OXMSG parser for MSG files
./target/release/tsp.exe --oxmsg ./msgs/

# Run all quality gates
cargo fmt --check && cargo check && cargo clippy --all-targets --all-features -- -D warnings && cargo test
```

## Project Status


| Milestone | Status       | Description                    |
| --------- | ------------ | ------------------------------ |
| M0        | Complete     | Architecture, ADRs, governance |
| M1        | Complete     | PST feasibility spike (P1-P4b) |
| M2        | In Progress  | MSG adapter implementation     |
| M3        | Not Started  | Semantic normalization         |
| M4        | Not Started  | Markdown archive engine        |


### M2: MSG Adapter Implementation

Two MSG parsing approaches are available:

1. **`msg_parser` (default)**: Mature crate, used for production MSG parsing
2. **CFB-based parser (`--oxmsg`)**: Custom MS-OXMSG parser using `cfb` crate

The CFB-based parser provides:

- Full property value decoding (privacy-safe)
- Fixed-length property stream decoding
- Sub-storage traversal (attachments, recipients)
- Named property resolution
- Cross-verification output format
- Comprehensive test coverage

Use `--oxmsg` to opt into the CFB-based parser for verification and comparison purposes.

## Features

### Privacy-Safe Diagnostics

All diagnostic output is carefully designed to **never expose PII**:

- No file paths in error messages
- No folder names
- No message subjects
- No sender/recipient addresses
- No message body content
- No attachment filenames
- Structural counts only
- Property presence/absence only
- Numeric property values (safe types only)
- String/binary property lengths (never content)

### Loss-Aware Design

Teaspoon follows a **no-silent-loss** principle:

- Every failed read is counted
- Every missing property is tracked
- Every decompression error is reported
- Unknown property types are flagged, not ignored

### Deterministic Output

- Sorted file scanning (deterministic order)
- BTreeMap for property tracking (sorted iteration)
- No randomness in output
- Reproducible across runs

## Output Format

### PST Diagnostic Output

```
ipm_subtree=ok
inventory=privacy_safe
input_kind=pst
ipm_subtree=opened
folders=9
messages=49
message_open_errors=0
folder_open_errors=0
property_values=3674
message_class class=IPM.Note count=49
bodies_plain=49
bodies_html=45
bodies_html_native=45
bodies_html_via_rtf=0
bodies_rtf=4
rtf_decompression_errors=0
rtf_decompressed_bytes_total=12345
messages_with_recipients=40
total_recipients=85
max_recipients_on_a_message=12
messages_with_attachments=20
total_attachments=65
...
```

### MSG Diagnostic Output (msg\_parser)

```
inventory=privacy_safe
input_kind=msg
files_scanned=29
subdirectories_skipped=3
open_errors=0
message_class class=IPM.Note count=29
bodies_plain=29
bodies_html=27
bodies_html_native=0
bodies_html_via_rtf=27
bodies_rtf=29
rtf_decompression_errors=0
rtf_decompressed_bytes_total=1787066
messages_with_recipients=29
recipients_to=29
recipients_cc=6
recipients_bcc=1
max_recipients_on_a_message=6
messages_with_attachments=11
total_attachments=29
...
```

### MSG Diagnostic Output (CFB parser with --oxmsg)

```
inventory=privacy_safe
input_kind=msg_oxmsg
files_scanned=29
subdirectories_skipped=3
open_errors=0
total_entries=2647
has_properties_stream=29
properties_stream_bytes_total=53504
property_streams_total=2363
property_values_decoded=2363
fixed_length_properties_decoded=128
attachment_storages_total=30
recipient_storages_total=37
has_named_property_storage=29
named_property_storage_bytes=4096
named_properties_parsed=29
attachment_storages_inspected=30
recipient_storages_inspected=37
attachment_property_streams=84
attachment_properties_decoded=84
recipient_property_streams=112
recipient_properties_decoded=112
unrecognized_entries_total=62

--- Property ID Coverage ---
files_with_message_class_prop=29
files_with_subject_prop=29
files_with_body_prop=29
files_with_html_body_prop=27
files_with_rtf_prop=29

--- All Property IDs Seen ---
property_id id=0x0002 count=1
property_id id=0x0003 count=1
property_id id=0x001A count=1
...
```

## Quality Gates

All code must pass:

```bash
# Format check
cargo fmt --check

# Compile check
cargo check

# Clippy with warnings as errors
cargo clippy --all-targets --all-features -- -D warnings

# Unit tests
cargo test

# Release build
cargo build --release
```

## Architecture

See `docs/ARCHITECTURE.md` for detailed architecture documentation.

### Key Design Principles

1. **Format Independence**: PST and MSG adapters both feed a common normalized representation
2. **Loss Transparency**: Failed reads and missing data are explicitly reported, never silently dropped
3. **Durable Archive**: Output is versioned, reproducible, and designed for long-term storage
4. **Privacy First**: Diagnostic output never exposes PII
5. **Standalone Operation**: No Classic Outlook, COM, or MAPI dependencies
6. **Evidence-Based**: Capabilities are only claimed after real-data verification

## Dependencies


| Crate            | Version | Purpose                             |
| ---------------- | ------- | ----------------------------------- |
| `anyhow`         | 1.x     | Error handling                      |
| `clap`           | 4.x     | CLI argument parsing                |
| `outlook-pst`    | 1.2.0   | PST file parsing                    |
| `msg_parser`     | 0.3.x   | MSG file parsing (default)          |
| `compressed-rtf` | 1.0.x   | RTF decompression (MS-OXRTFCP)      |
| `cfb`            | 0.14.x  | CFB container parsing (for --oxmsg) |


## Verification

See `docs/verification/` for detailed verification results:

- `m1-results.md` - PST feasibility spike results
- `m2-results.md` - MSG spike results
- `oxmsg-results.md` - CFB-based MS-OXMSG parser results

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make changes with test coverage
4. Run quality gates
5. Submit PR with verification evidence

## License

MIT OR Apache-2.0

## Version History

See GitHub releases for full history.

### v0.1.7 (Latest)

- Added CFB-based MS-OXMSG parser groundwork
- Added `cfb` and `msg_parser` dependencies
- Added `--oxmsg` flag for experimental CFB parser

### v0.1.0 - v0.1.6

- M1 completion: PST feasibility spike
- P1-P4b: Inventory, privacy-safe diagnostics, fixture verification
