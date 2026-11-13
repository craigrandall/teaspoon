# Architecture

## Intent

`teaspoon` is a format-independent Outlook item mining tool.

The key boundary is:

```text
PST / MSG serialization
        |
        v
format adapter
        |
        v
normalized Outlook item
        |
        +--> Markdown
        +--> metadata
        +--> attachment archive
        +--> diagnostics
```

PST and MSG must not leak into the renderer or archive writer.

`architecture.mmd` and `pipeline.mmd` in this folder draw this target
architecture. They describe where the project is going, not what the code
does today. The sections below record the boundaries milestone by milestone, so an older
section can describe a state that a later one supersedes (the later section says so);
"Code layout" at the end is current.

## Domain boundary

The eventual normalized model should represent:

- source/provenance
- folder location where applicable
- message class
- message properties
- recipients
- plain/HTML/RTF bodies
- attachments
- embedded messages
- named properties
- extraction diagnostics

The raw property bag is intentional: Outlook's property model is larger than the subset required by Markdown.

## M1 boundary

M1 intentionally stops at:

```text
PST
 |
 +-- open store
 |
 +-- IPM subtree
 |     |
 |     +-- folders
 |           |
 |           +-- hierarchy
 |           +-- contents
 |                 |
 |                 +-- message entry IDs
 |                 +-- message properties
 |
 +-- deterministic inventory
```

M1 does not create the final normalized domain model. That prevents a spike implementation from prematurely becoming the architecture.

## M2–M3 boundary: the MSG adapter

The MSG side started in M2 as a diagnostic built on the `msg_parser` crate,
and became a custom adapter in M3. Since M3f the custom adapter is the
production `.msg` path; `msg_parser` is used only as the oracle behind
`--verify` (and, since M4d, `--verify-envelope`).

```text
.msg file
 |
 +-- MS-CFB container            (generic `cfb` crate)
 |
 +-- MS-OXMSG naming layer       (teaspoon)
 |     |
 |     +-- entry classification  (every entry accounted for, or counted)
 |     +-- properties streams    (typed fixed values, entry shapes)
 |     +-- value streams         (UTF-16LE, PT_STRING8 via a code page
 |     |                          chain, binary, CLSID)
 |     +-- named-property map    (GUID / numeric / string names)
 |     +-- recipient, attachment, and embedded-message storages
 |
 +-- extraction layer            (message class, bodies, recipients,
 |                                attachments, embedded message class)
 |
 +-- deterministic inventory     (same report shape as the PST side)
```

The extraction layer reads real content into memory to compute booleans and
counts, and prints none of it. Like M1, this boundary stops before the
normalized domain model: the MSG adapter decodes the values such a model
would be built from. (Since M4c and M4d, the export path assembles some of
them into a small model; see the sections below.)

## M4b boundary: planning an export without writing one

M4b adds the part of the export that needs no I/O: deciding every name and
path before anything is written.

```text
.pst / .msg input
 |
 +-- source reader               (source_pst.rs / source_msg.rs)
 |     reads names, times, attachment names, embedded messages into a
 |     source tree, plus a content-free census of what it saw
 |
 +-- naming rules                (naming.rs: sanitize, normalize, truncate to
 |                                a UTF-16 unit budget, reserved names,
 |                                collision key)
 |
 +-- export planner              (plan.rs: pure; source tree + Policy -> planned
 |     entries with final names, flags, counters)
 |
 +-- plan verifier               (plan.rs `verify_plan`: collisions, over-budget
 |                                paths, invalid names, unit mismatches)
 |
 +-- --dry-run report            (dry_run.rs: counts only)
```

The planner and the naming rules depend on no adapter module; the source
readers are the only code that knows about PST or MSG here. Nothing is
written by any code in this boundary.

## M4c boundary: the first writer (`.msg` input, plain-text body)

M4c consumes the same plan and writes an archive. It is a walking skeleton:
one thin path from a `.msg` input to files on disk, with the pieces that will
grow later already separated.

```text
.msg file / directory
 |
 +-- source_msg.rs   source tree + map: message source ID -> its .msg file
 |
 +-- plan.rs         the same plan `--dry-run` reports; export refuses a plan
 |                   that breaks a gate (exit code 2, nothing written)
 |
 +-- source_msg.rs   `read_message_content`: re-reads one message when the
 |                   writer needs it -> model.rs `MessageContent`
 |
 +-- archive.rs      pure rendering, no I/O: message.md, metadata.json,
 |                   folder.json (draft schema "0.1-draft")
 |
 +-- export.rs       render everything in memory -> preflight against the
                     target (counts only) -> consent -> write through
                     <out>/.tsp-tmp -> read back -> content-free report
```

- `model.rs` began as the seed of the normalized model (M4b-4): subject, Internet message ID, time, the plain-text body, and which other body forms exist. It imports no adapter module.
- `archive.rs` has no I/O, so what is written is testable byte for byte (golden files are committed under `tests/golden/`).
- `export.rs` never overwrites or deletes what it did not generate: the target must be absent, empty, or a directory whose root `folder.json` was written by `tsp` for the same source; its own files are replaced only with consent (`--overwrite` or a yes at the prompt).
- Each file is replaced atomically (written under a short numeric name in `.tsp-tmp`, then renamed). The archive as a whole is not all-or-nothing: the root `folder.json` is first written as `incomplete` and replaced with the `complete` version last, so an interrupted run is recognizable.
- As of M4c, not exported: attachments and embedded messages (added in M4f), HTML and RTF bodies (added in M4e-2b), the remaining message properties, and `.pst` input. The sections below record each addition.
- Since M4h-0 `export.rs` no longer renders everything in memory first: it stages each file as it is rendered and keeps only a manifest (see `stage.rs` and the M4f-1 section). `read_message_content` in the diagram above is now `open_message`; the former survives only as a test helper.

## M4d boundary: the envelope

M4d adds who a message was from and to, and when, through the same path. The
extraction is a separate adapter module so that the pure model and the pure
renderers stay free of MSG details.

```text
.msg file
 |
 +-- oxmsg_envelope.rs   reads, from the message's properties and its recipient
 |                       storages: sender, sent-representing identity, To/Cc/Bcc
 |                       recipients (display name, address type, address, and
 |                       SMTP address kept separate), submit and delivery times,
 |                       importance, sensitivity, conversation topic and index,
 |                       transport headers; strings via the code page chain;
 |                       nothing derived or repaired
 |
 +-- source_msg.rs       `read_message_content` -> model.rs `Envelope`
 |                       (with `Address` and `Recipient`) inside `MessageContent`
 |
 +-- archive.rs          pure rendering: a header list in message.md (each
                         value in a code span; the SMTP address preferred and
                         an Exchange distinguished name never shown) and an
                         `envelope` object in metadata.json (stored ticks plus
                         one UTC string; stored integers for importance and
                         sensitivity)
```

- Property identifiers, how well each is confirmed, and the extraction rules are in [`plans/m4d-envelope-properties.md`](plans/m4d-envelope-properties.md).
- A recipient row that is not To, Cc, or Bcc (the originator row, an unknown type, an unreadable type) is counted in `recipients_unlisted` and not listed.
- `verify_envelope.rs` is the envelope's check: it runs the same extraction and `msg_parser` over the same files and prints match/mismatch counts for the subject, sender, and recipients. It does not touch the export path.
- The status reason `envelope_not_extracted` was replaced by `other_properties_not_preserved`: the envelope is now written, but the rest of the property bag still is not.

## M4e-1 boundary: de-encapsulation (built in v0.1.26)

M4e-1 adds the first body-conversion step, kept pure and outside the writer for now.

```text
PidTagRtfCompressed (bytes)
 |
 +-- compressed-rtf          MS-OXRTFCP decompression (existing)
 |
 +-- rtf_deencap.rs          pure: recognize (10-token rule) -> tokenize -> interpret
 |                           (htmltag groups, htmlrtf suppression, font code pages,
 |                            \uN with fallbacks, skipped destinations) -> UTF-8 HTML
 |                           plus content-free diagnostics; bounded depth and output
 |
 +-- verify_deencap.rs       --verify-deencap: the recovered HTML against msg_parser's
                             html_from_rtf(), graded; recognition rule against the
                             whole-document \fromhtml1 search
```

- `rtf_deencap` imports only the `PT_STRING8` decoders from `oxmsg_decode`; it reads no file and knows no adapter type. At M4e-1 nothing in the export called it; since M4e-2 `body.rs` does.
- The recovered text is UTF-8, so a `charset=` declaration inside it is stale; the diagnostics flag one.

## M4f-1 boundary: by-value attachments, streamed in chunks (built in v0.1.34)

M4f-1 adds attachment bytes to the export without letting memory depend on their size.

```text
.msg file
 |
 +-- source_msg.rs   `open_message`: the content (model.rs `MessageContent`, with one
 |                   `AttachmentInfo` per attachment: method, content ID, declared data
 |                   length; never bytes) plus `MsgAttachments`, a handle that opens one
 |                   attachment's data stream at a time
 |
 +-- export.rs       per message: stage each by-value attachment, then render
 |                   metadata.json (it records every attachment), then message.md
 |
 +-- stage.rs        `add_stream`: copy the source in 256 KiB chunks to the staging
                     directory, hashing as it writes; the size and SHA-256 recorded are
                     those of what was written
```

- The model holds descriptors, not bytes. The bytes are read from the source at staging time, one attachment at a time, so memory is one chunk plus the message's bodies, whatever the attachment's size.
- `add_stream` takes any `Read`, so nothing in the stager or the model names the container format; a PST reader (M4i) can supply a stream the same way.
- A failure of the source (a read error, or a length other than the one the container declares) removes the partial staged file, adds nothing to the manifest, and is returned to the export, which records the attachment as `failed` and goes on. A failure to write to staging is an error and stops the run.
- The attachments are staged before `metadata.json` because it records each one's size and SHA-256, which are known only after the copy.
- `stream_len` reads a stream's length from its directory entry without reading the stream (the export's declared length); `drain_stream_len` reads a stream through in chunks and counts the bytes (the diagnostics' zero-byte check), so an unreadable stream is still noticed.
- Embedded messages, OLE objects, and by-reference attachments are recorded as not extracted, with a reason (embedded messages until M4f-2a, which writes them as nested message directories; see the next section). `cid:` references in `message.md` stayed as written until M4f-2b (see the last of these sections).
- Verified so far: synthetic tests, and a synthetic memory gate (peak working set flat at 28 MB for attachments of 1, 50, and 200 MB); the real-corpus runs came with M4f-2 (next section).

## M4f-2 boundary: embedded messages as nested folders, and the payload differential (built in v0.1.35)

M4f-2a writes an embedded message the way the export writes a top-level one, one level below; M4f-2c checks the attachment bytes against an independent reader.

```text
.msg file
 |
 +-- source_msg.rs        `open_message` -> content + `MsgAttachments` (owns the open file);
 |                        `open_embedded(i)` -> `EmbeddedOpen`: the embedded message's content
 |                        (body forms, envelope with its own recipients, attachment
 |                        descriptors) and a `MsgAttachments` that borrows the parent's file,
 |                        so only one level is read at a time
 |
 +-- oxmsg_extract.rs     `read_body_forms_at`, `extract_envelope_at`: the same readers,
 +-- oxmsg_envelope.rs    given the storage the message lives in and its properties header
 |                        length (32 for the file's message, 24 for an embedded one)
 |
 +-- export/nested.rs     `stage_message`: attachments, then message.md, metadata.json, and the
 |                        kept body files, for a top-level or an embedded message;
 |                        `stage_embedded`: recurse for an embedded attachment, or write only
 |                        a metadata.json for one at the planner's depth cap
 |
 +-- verify_attachments.rs  --verify-attachments: SHA-256 and length of each top-level
                            by-value attachment through the export's own reader, against
                            msg_parser's payload
```

- The planner is unchanged: it already planned embedded messages as directories under `attachments/`, their attachments below them, a depth cap (three levels in full), and room for a `metadata.json` at the cap. The export now follows the plan.
- The recursion is bounded by the plan, not by the source: the source reader reads embedded messages up to eight levels deep, and the planner stops at three.
- An embedded message is a message in the archive's terms: it gets `message.md` and `metadata.json` from the same renderers, and only `kind` (`embedded_message`) differs. A failure to open one is recorded on its parent's attachment record and does not stop the export.
- The counters of top-level messages keep their meaning; embedded messages have their own (`export_embedded_messages_*`), and `export_attachments_*` include attachments inside embedded messages.
- `--verify-attachments` is outside the export path and reads through the same `open_data`. It does not open embedded messages, because `msg_parser` does not.
- Verified so far: synthetic tests (one level, duplicate subjects, the depth cap, an unreadable storage, the reader directly) and the corpus (3 of 3 embedded messages written, 217 files, 0 read-back mismatches, 30 of 30 attachment payloads equal to `msg_parser`'s). The depth cap and the unreadable case have not run on real data.

## M4f-2b boundary: `cid:` links resolved once the attachments are staged (built in v0.1.36)

M4f-2b makes the Markdown of `message.md` point at the files the archive holds. The order matters: a link can be resolved only when it is known which attachments were extracted and under what name, which is known only after they are staged.

```text
one message (top-level or embedded)
 |
 +-- archive.rs `body_outcome`        select, preprocess, convert: the Markdown, and how many
 |                                    `cid:` references it holds (body.rs `find_cid_references`)
 |
 +-- export/nested.rs                 stage the attachments: one record each (content ID,
 |   `stage_message`                  status, the file name actually written)
 |
 +-- archive.rs                       the Markdown and the records -> the rewritten Markdown (or
 |   `link_cid_references`            none, when nothing was rewritten), the counts, and
 |                                    `inline` set on the records
 |
 +-- archive.rs                       the records and counts into metadata.json; the rewritten
     `message_metadata`,              Markdown, in place of the converted one, into message.md
     `render_message_md_linked`
```

- The resolving is pure: it takes text and records and returns text, records, and counts, with no I/O, so it is tested without a file. The scanner, the comparison of content IDs, and the encoding of a name into a link are in `body.rs`, beside the existing count of references, which now uses the same scanner.
- Only `message.md` is rewritten. The kept `body.html`, `body.rtf`, and `body.txt` are written from the original bytes and are never given the rewritten text, so a better rule later can be applied again from the kept files.
- A link is rewritten only when exactly one extracted attachment of the same message has the content ID. A link that matches none, only attachments that were not extracted, or more than one extracted attachment is left as written and counted under that reason; nothing is guessed from a name or a position.
- An embedded message goes through the same function, against its own attachments, because it is staged by the same `stage_message`. Its counters are kept apart from the top-level ones (`export_embedded_cid_*`), so the identities among the top-level counters hold.
- `render_message_md` remains, as a test-only wrapper of `render_message_md_linked` without a rewrite.
- Verified so far: synthetic tests of each rule and of a real export, and the corpus (26 of 26 references rewritten, each resolving to a file the same message records as extracted, checked by a counts-only script outside the crate). A `cid:` link inside an embedded message has not been seen on real data.

## M4i-0 boundary: the PST password check (built in v0.1.39; exit status 2 for a refusal since v0.1.40)

A PST can carry a password, which the format treats as advisory: a CRC-32 of it is stored in the message store, and the data can be read without it. `tsp` checks it before it reads anything else from the file, and refuses a protected PST whose password is missing or wrong.

```text
PST file
  |
  +-- outlook_pst::open_store          called by pst.rs (diagnostic) and source_pst.rs (planner),
  |                                    inside shared::contained since v0.1.42
  |
  +-- pst_password::enforce            reads PidTagPstPassword (0x67FF) from the store properties
  |     |                              (absent or zero: unprotected) and compares a CRC-32 of the
  |     |                              password in TSP_PST_PASSWORD with the stored value
  |     +-- evaluate                   pure: stored value + supplied text -> NotProtected, Verified,
  |                                    Missing, Wrong, or UnsupportedEncoding (no store, no I/O)
  |
  +-- refused: a PasswordRefusal error (exit code 2), nothing on standard output
  +-- allowed: pst_password= is reported (the diagnostic prints it first, the dry run carries it
               in SourceCensus) and the walk continues
```

- One function guards both entry points, so the diagnostic and the planner cannot disagree about which files they read. The `.msg` path never calls it.
- `evaluate` is a pure function over the stored value and the supplied text, so the decision table is unit-tested without a PST. `enforce` only adds the property read and turns a refusal into a typed error, `PasswordRefusal`; `main` maps that error to exit code 2 (since v0.1.40) and every other error to exit code 1.
- The scheme (reflected CRC-32, polynomial 0xEDB88320, initial value 0, no final XOR, over ASCII bytes) is written in `pst_password.rs` and recorded in `ADRs/pst-password-protection.md`. The MS-PST pages do not state it, so it was found against a real protected PST.
- Nothing derived from the password, including the stored hash, is printed or put in an error message. The password is read from the environment only.
- The PST header's block-encoding method is a separate matter and is not reported.
- The code is verified against one real protected PST (see `docs/verification/m4-results.md`). The PST export (M4i-2, drop A) calls the same function from `PstSource::open`, and so does the attachment spike.

## M4i-1 boundary: the attachment-reach spike (release v0.1.41, run 2026-11-05 and 2026-11-06)

The spike answers one question before any PST export code exists: can attachment bytes be read from a Unicode PST through the public `outlook-pst` API, correctly, and at what memory cost? It is a temporary, read-only, counts-only mode (`--spike-attachments`, `src/spike_pst_attachments.rs`). The reading it does lives in `src/pst_attachments.rs`, which the PST export reuses.

```text
PST file
  |
  +-- pst_attachments::open_unicode_pst    UnicodePstFile::open; an ANSI PST is told apart and refused
  +-- UnicodeStore::read                   a concrete store (the dyn Store reaches no attachment)
  +-- pst_password::enforce                the same rule as the diagnostic and the planner
  |
  +-- walk the folders (dyn open_folder), and per message:
        UnicodeMessage::read(store, entry ID, None)
          |
          +-- attachment_rows          the attachment table: row ID (taken as the attachment's node),
          |                            method, size, content ID
          +-- open_attachment          UnicodeAttachment::read(message, node, None), then data():
                Binary                 hashed in memory (SHA-256), then dropped
                error or no data       counted by method and by the crate's error variant name
```

- The crate reads every property of an attachment, the payload included, whatever property filter is passed, and holds a by-value payload about twice while the attachment object lives. The spike drops each attachment after hashing it.
- Output is counts only. With `--spike-hash-file FILE` it also writes one line per attachment row (ordinals, method, kind, size, SHA-256; no names) to a new file, and refuses to do so for a PST over 256 MiB, so that a list for a large mailbox cannot be made by accident.
- Error labels are the variant name of the crate's error (plus a bare property type or number), never a payload; the by-value arm raises the embedded-message variants, so a label is read beside the method.
- Verified by the owner's runs: on the tester the by-value payloads of a PNG (seven identical copies) and a JPG equal Outlook's Save As on size and SHA-256, the row ID is the attachment's node (0 method or size disagreements), and the 3 embedded-message and 2 OLE attachments fail with `AttachmentMethodNotFound`; on `crkb.pst` all 7,016 attachments read (5,991,077,209 bytes in 116 s at a 91 MB peak working set under `measure-peak.ps1`). Details and limits: `docs/plans/m4-plan.md`, M4i.

## M4i-2 boundary: exporting a PST (drop A, release v0.1.41; built and run on `tsp-tester.pst` on 2026-11-09)

A PST is exported through the same lazy, staged, streaming exporter as `.msg` input. Two seams change; consent, preflight, staging, read-back, the tree hash, and naming do not.

```text
tsp <pst> --out <dir>
  |
  +-- PstSource::open                  Unicode store; password enforced (a refusal is exit code 2,
  |                                    before anything is staged); an ANSI PST is refused (exit code 1)
  +-- PstSource::build_tree            source_pst::build_pst_tree_from, the builder the dry run uses
  |                                    (one store for the plan and the export) -> SourceFolder + census
  +-- plan gates, preflight, consent, staging, read-back        unchanged (export.rs, stage.rs)
  |
  +-- per planned message:  ExportSources::Pst -> PstSource::open_message   (inside run_contained)
  |     content   subject, Internet message ID, time, bodies as stored, attachment descriptors
  |               (the envelope is empty until drop B)
  |     reader    AttachmentReader::Pst: open_data(i) -> DataOpen::Stream (a cursor over the crate's
  |               buffer, copied to staging in chunks) | Missing | ReadFailed
  |               embedded-message and OLE rows are never opened: recorded not_extracted from their
  |               method (embedded_message_unreadable, ole_object)
  |     identity  pst_identity in metadata.json (node ID, provider UID) and folder.json (+ entry ID)
```

- `export_source.rs` is the seam: `ExportSources` (the `.msg` file map or the open PST), `AttachmentReader` (an enum over the `.msg` reader and the PST reader), and `DataOpen`. The enum, not a trait object, keeps the nested-message code on its existing concrete reader.
- A by-value read failure is recorded `failed` (`data_stream_missing` for a null or missing data property, `data_read_failed` otherwise); the crate's error label is counted in the stdout report (`export_pst_attachment_read_error`) and is not written to the archive. No vocabulary is added before M4g closes it.
- PST strings are read through `pst_text.rs`: a `PT_STRING8` value from its raw bytes through the chain the `.msg` path verified (message code page, Internet code page, Windows-1252), with the loss counted. The crate's own `to_string()` reads each byte as Latin-1 and is not used on a `String8` value. The planner reads names the same way, so a planned name and the exported text agree.
- The new metadata fields (`pst_identity`) are omitted for `.msg` input, and the PST-only report keys print only for a PST export, so `.msg` output should be byte-identical to the build before drop A except for the version the root marker records. Measured on 2026-11-09 at `Cargo.toml` 0.1.41: the counts equal the values recorded earlier (217 files and 32 attachments extracted, 2 not extracted, since v0.1.35; 26 `cid:` links rewritten since v0.1.36; 0 read-back mismatches), no metadata file holds `tilde_replaced`, and the tree hash is `b2902cb8…9df6` against v0.1.40's `6ea3c22c…de35`. That is consistent with a difference of the version string only; it was not shown by building the v0.1.40 sources at version 0.1.41.
- Containment nests here. `PstSource::open_message` runs `read_message` inside `shared::run_contained`, and `read_message` decompresses RTF through `shared::decompress_rtf_bounded`, which calls `run_contained` again. `run_contained` is reentrant on one thread (a thread-local depth counter; only the outermost call takes the mutex and swaps the panic hook), which the first tester export needed: with the earlier plain mutex it deadlocked on the first message with an RTF body (2026-11-09). The depth is per thread, so containment does not extend across threads: a thread that a contained call spawns and waits for must not call `run_contained` (today's one spawn, in `body::convert_html_isolated`, does not).
- Since v0.1.42 the rest of the PST path is contained too (the review finding of 2026-11-09, closed). `shared::contained(f, what)` is `run_contained` for a single call whose caller can only fail as a whole: a panic inside `outlook-pst` becomes an error that names the step, with fixed text and never the input path or any content. It wraps `UnicodePstFile::open` and the ANSI probe (`pst_attachments::open_unicode_pst`), `UnicodeStore::read` and the password check (`PstSource::open`, `build_pst_tree`, `run_pst_diagnostic`), opening the IPM subtree, and the root walk of the planner and the diagnostic. Inside those walks each message and each subfolder runs under `run_contained` on its own, and a panic is counted as an open error (`open_errors` in the planner; `message_open_errors` and `folder_open_errors` in the diagnostic) while the walk goes on; a subfolder whose own table read panics is dropped with the messages already read from it. The per-message and per-attachment reads of the export were already contained. Left uncontained: the temporary spike module, and the in-memory construction of entry IDs. Verified by two unit tests of `contained` and by unchanged outputs on good input; no fixture makes the crate panic, so the counted paths have not run on real data.
- A `~` in any source name becomes `_` before planning (naming rule N3, amended 2026-11-09, flag `tilde_replaced`; `naming::sanitize_component`, checked by `is_valid_component`). Without it, a bare `Screen~1.jpg` next to `Screen~1 (02).jpg` can resolve to the 8.3 short name Windows gave the other file, and one file replaces the other. A side effect: the planner sanitizes before it tests for tool-special names, so the `~$` case of rule N6 can no longer match (`~$x.docx` is exported as `_$x.docx`).
- The names, and so the tree hash, depend on the length of the output root, because the path budget is taken from it (naming ADR, root-length note; export rules L2a). Two exports of one PST are byte-identical at roots of the same length: at `C:\o.tester` a repeat export wrote 0 files and printed the same hash, while `C:\o.tester2`, one unit longer, printed another.
- Not in drop A: the PST envelope (drop B), class-specific handling of non-mail items, skipping or counting associated items and search folders, and ANSI.
- Run on `crkb.pst` (6,508 messages, 7,016 attachments; 2026-11-11): 28,222 files, 0 read-back mismatches, a peak working set of 107 MB (the spike's was 91 MB, so memory does not grow with the mailbox), and one tree hash for a fresh export, a repeat, and a fresh export at an equal-length root. A native `body.html` is the stored bytes and a recovered one is UTF-8 text (`archive.rs`: the notes `as_stored` and `recovered_from_rtf_charset_declaration_stale`); 29 messages carry RTF text in a code page with no decoder (26 files declare Windows-1256), whose non-ASCII bytes the de-encapsulation replaces and counts, with the received RTF kept as `body.rtf`. A repeat export stages every file before finding it identical, so it costs more than a fresh one (919 s against 225 to 337 s).

## Verification structure

`tsp --verify` runs the custom MSG path and `msg_parser` over the same files
and prints match and mismatch counts per field, never the values compared.
It also runs the custom path's structural accounting and prints its gate
counters, which must all read 0, with a single `structural_gate_violations`
total; when any gate is nonzero it also prints the structural breakdown for
triage. Disagreements are triaged against the Microsoft specifications, not
settled by majority vote (see
[ADR: Microsoft specifications as normative authority](../ADRs/microsoft-specifications-as-normative-authority.md)
and [ADR: independent differential verification](../ADRs/independent-differential-verification.md)).

`tsp --verify-envelope` does the same for the envelope: the custom path's
subject, sender, and recipients against `msg_parser`'s, field by field,
reporting for each email which of the custom path's two properties
(`PidTagEmailAddress` or the SMTP address) equalled the oracle's string. The
fields `msg_parser` does not expose (sent-representing, times, importance,
sensitivity, conversation fields, transport headers) are reported only as
presence counts, so they have no independent check; they rest on the
specification and the unit tests. Like `--verify`, it scans a directory
non-recursively.

`tsp --verify-attachments` (M4f-2c) compares the bytes of every top-level
by-value attachment, read through the export's own reader, with
`msg_parser`'s payload: SHA-256 and length, by position in storage order,
with an order-free comparison beside it, counts only. On the corpus all 30
by-value attachments match. It does not cover the attachments inside
embedded messages, which `msg_parser` does not open.

After M4f-2b a counts-only PowerShell check (`verify-cid-links.ps1`, kept outside the crate) walks an exported tree and, for every `message.md` at any depth, checks that each link to `attachments/` resolves to a file on disk that the `metadata.json` beside it records as an extracted attachment, that no `cid:` link is left, that the links equal the sum of the `rewritten` counts, and that no attachment is marked `inline` without a content ID. On the corpus: 26 links, all resolving, 0 violations. It shows that a link points at a recorded extracted file, not that the match by content ID was the right one.

The PST side has no `libpff` or `libpst` comparison yet. The comparisons made by hand with Outlook are the attachment payloads of the M4i spike (2026-11-05: the tester's PNG, seven identical copies, and its JPG equal the payloads read, on size and SHA-256) and one manual comparison of the folder tree with Outlook's Folder Size dialog (35 subfolders in both; see `docs/verification/m4-results.md`). The export's own checks on the tester (2026-11-09) are the read-back of every written file (0 mismatches), a repeat export that writes nothing, and a longest path on disk equal to the plan's (253); on `crkb.pst` (2026-11-11) the same checks, a tree hash that is the same across three exports, and `verify-cid-links.ps1` over the exported tree (2,973 links, all resolving). The exported tree has no independent reader, and no independent count of `crkb.pst`'s messages and attachments exists (Outlook's object model failed to load the store). The PST password check was verified differently: against a real protected PST, where the stored hash equals the hash of the known password under the scheme in the ADR (an independent computation agreed), a missing and a wrong password are refused, and the right one is accepted with the census unchanged.

The planner is verified differently: `verify_plan` is a gate over every plan
(its violation count prints under `--dry-run` and gates `export`), unit and
property tests cover the naming and planning rules, and `verify-split.ps1`
checks that a change leaves the existing outputs byte-identical to an earlier
tag. The writer is verified by synthetic tests (golden files, determinism,
the overwrite matrix) and was run on real corpus messages at v0.1.23.1; the
envelope output was examined on a couple of exported messages at v0.1.25.1,
and a full corpus export for that tag has not been reported.

## Code layout

The code lives in `src/`, one module per seam:

- `cli` — arguments and input classification; `main` — dispatch and the exit status (since v0.1.40 `run` returns the error and `main` prints it and maps a `PasswordRefusal` to exit code 2 and every other error to 1).
- `shared` — vocabulary, the encapsulated-HTML check, shared counters, the traversal depth limits, `is_msg_file`, the one bounded and panic-contained RTF decompression, and `run_contained` (reentrant on one thread since v0.1.41, so a helper may nest it inside a wider contained region; not across threads), and `contained` (v0.1.42: `run_contained` for a single call whose caller can only fail as a whole, an error that names the step instead of a panic).
- `pst` — the PST diagnostic; `msg_report` — the shared MSG report.
- `pst_password` - the PST password check (v0.1.39): reads `PidTagPstPassword`, compares a CRC-32 of the password in `TSP_PST_PASSWORD` with it, and refuses a protected PST whose password is missing or wrong, with a typed `PasswordRefusal` that `main` maps to exit code 2 (v0.1.40); called by `pst` and `source_pst` right after the store is opened.
- `oxmsg_classify`, `oxmsg_decode`, `oxmsg_structure`, `oxmsg_extract` — the custom MS-OXMSG parser, from naming conventions through extraction; `oxmsg_envelope` — the envelope extraction (M4d).
- `verify` — the `--verify` comparison and structural gates; `verify_envelope` — the `--verify-envelope` comparison (M4d); `verify_deencap` — the `--verify-deencap` comparison (M4e-1).
- `rtf_deencap` — the pure MS-OXRTFEX de-encapsulation (M4e-1).
- `naming`, `plan` — the pure naming rules and export planner (M4b); since v0.1.41 a `~` in a name becomes `_`, and the planner's special-name test runs after that.
- `source_msg`, `source_pst`, `dry_run` — readers that build the planner's source tree, and the `--dry-run` report (M4b); `source_msg` also opens message content for export (the body forms, the envelope, and a descriptor for each attachment) together with a handle that streams one attachment's data at a time and opens an embedded attachment as a nested message through that handle (M4f-2a).
- `model`, `archive`, `export` — the model so far (message content and envelope), the pure renderers, and the writer with its preflight and consent rules (M4c, extended by M4d).
- `stage` — the staging area of the streamed export (M4h-0): files are written to `.tsp-tmp` as they are rendered, only a manifest is kept, and `add_stream` copies attachment data in chunks (M4f-1).
- `body` — the pure body pipeline (M4e-2a); since M4f-2b it also holds the scanner for `cid:` references, the comparison of content IDs, and the link from a message to one of its extracted attachments.
- `tests` — most of the older unit tests; the newer modules keep theirs beside their code.
- `export/nested` — a child module of `export` (M4f-2a): stages one message's files for both top-level and embedded messages and writes embedded messages as nested message directories, to the planner's depth cap.
- `verify_attachments` — `--verify-attachments` (M4f-2c): the byte-level comparison of extracted attachments with `msg_parser`.
- `verify_bodies` — `--verify-bodies` (M4e-2c): the content-preservation check of the HTML-to-Markdown conversion, in order, counts only.
- `pst_attachments` — the concrete route to a PST attachment's payload (M4i-1): the attachment-table rows, `open_attachment`, content-free error and method labels, the shared Unicode open that refuses an ANSI PST; reused by the PST export.
- `pst_text` — PST text (M4i-2): string values read through the verified `PT_STRING8` chain, with counted loss.
- `pst_export` — the PST side of the export (M4i-2): the open Unicode store, the message opener, the attachment reader, and the identity records.
- `export_source` — the exporter's two source seams (M4i-2): `ExportSources`, `AttachmentReader`, `DataOpen`.
- `spike_pst_attachments` — `--spike-attachments` (M4i-1), temporary: counts and an optional per-attachment hash file for the tester.
- `bakeoff` — the HTML-to-Markdown bake-off harness (M4e-0, `--bakeoff-md`); the candidate converters compile only with `--features bakeoff`.
- `layout` — the archive's fixed names (`folder.json`, `message.md`, `metadata.json`, `attachments`). The planner derives its path-budget tails from their lengths, and the renderers, the writer, and the body pipeline's links use the same constants.

The README lists the contents of each module.

Dependency direction today: `shared` depends on nothing in the crate; the PST diagnostic and the `oxmsg_*` modules depend on `shared`; `verify` depends on the extraction and structure modules; `naming` and `plan` depend on no adapter module; `source_msg` and `source_pst` depend on their adapters and on `plan`; `model` depends on nothing; `archive` depends on `model` and `naming`; `export` depends on `archive`, `model`, `plan`, `naming`, `dry_run`, and `source_msg`. The envelope modules follow the same pattern: `oxmsg_envelope` is an adapter module that depends on `oxmsg_classify`, `oxmsg_decode`, `oxmsg_extract`, and `shared` and produces `model` types (it also takes the submit and delivery time property IDs from `dry_run`, where they were first defined, which is a small layering oddity: an adapter importing from the reporting module), and `verify_envelope` depends on `model`, `source_msg`, `dry_run` (for the subject-marker stripping), and `verify` (for the shared comparison tallies). Every item is `pub(crate)`, so the module boundaries are organizational, not yet a designed API. The blanket `#![allow(dead_code)]` that `naming.rs` and `plan.rs` carried is gone, so dead code is reported there like everywhere else. `layout` holds only constants and is imported by `plan`, `archive`, and `body`. `pst_password` depends only on `outlook_pst`; `pst` and `source_pst` call it, `main` imports its `PasswordRefusal` to choose the exit status, and `dry_run` imports its `PasswordOutcome` for the census, which puts one adapter-side type in the reporting module. The full normalized model (M4b-4) has not been written: bodies with raw bytes, the raw property bag, named properties, embedded messages, and diagnostics are not modelled yet (attachments are modelled as descriptors since M4f-1).
