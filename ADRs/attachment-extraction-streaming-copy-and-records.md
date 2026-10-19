---
status: "proposed"
date: 2026-10-18
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Attachment extraction: chunked copy, per-attachment records, and failure handling

*Status history: proposed 2026-10-18. The owner accepted the choices below as recommended on 2026-10-18, before M4f-1 was written; this text records them and counts as accepted when the owner commits it. The behavior is implemented for by-value attachments of top-level messages (M4f-1, tag `v0.1.34`) and was tested on synthetic input and measured with a synthetic memory gate; it has not been run on the real corpus (see Confirmation). Not decided here: embedded messages as nested message folders and the rewriting of `cid:` references to files (M4f-2), marking inline attachments, attachments read from a PST (M4i), and the closed status-reason vocabulary and the first numbered `schema_version` (M4g).*

## Context and Problem Statement

[Deterministic Markdown archive](deterministic-markdown-archive.md) gave each message folder an `attachments/` subfolder, and [Export naming, collisions, and path budgets](export-naming-collisions-and-path-budgets.md) plans every attachment's file name before anything is written. Until M4f-1 the export only counted attachments as not extracted. Writing their bytes raises four questions, each expensive to reverse because it fixes what an archive contains and how the writer is built: how attachment bytes get from the source to the archive (the owner's PST files are several gigabytes, and one attachment can be hundreds of megabytes); what is recorded about each attachment; what happens when one cannot be read; and what each attachment method (by value, by reference, OLE, embedded message) produces.

## Decision Drivers

- Memory must not depend on the size of an attachment, as it no longer depends on the size of the archive ([Export output posture](export-output-posture-consent-and-ownership.md), M4h-0).
- Nothing may disappear without a record: every attachment is accounted for, extracted or not ([Archive file contract](archive-file-contract-and-schema-evolution.md), rule 6).
- One unreadable attachment must not hide the others or abort a long export.
- What is recorded about a file must be what was written to disk.
- The names are known before the bytes (the plan), and output must stay deterministic.
- The model and the stager must not name the container format, so that a PST reader (M4i) can supply bytes the same way.
- The record's shape is part of a draft schema that M4g will freeze; it should not need a breaking change when embedded messages arrive.

## Considered Options

How the bytes move:

* **A. Copy the source stream to staging in fixed-size chunks, hashing as it is written (chosen).**
* B. Read one attachment whole into memory, then stage it as a whole file.
* C. Read all of a message's attachments into the model (as the bodies are).
* D. Copy from the source straight to the final path, without staging.

When an attachment cannot be read: **A. record it as failed and continue (chosen)**; B. abort the export; C. skip it without a record.

What is recorded: **A. one record per attachment, in storage order, extracted or not (chosen)**; B. records only for extracted attachments plus a count; C. a separate `attachments.json`.

## Decision Outcome

Chosen options as marked, because only they keep memory constant, account for every attachment, and let one bad attachment fail alone.

**Binding decisions:**

1. **What is written.** Each by-value attachment (`PidTagAttachMethod` 1) of a top-level message is written byte for byte as `attachments/<planned name>` in its message folder, under the name the plan assigned. Extracted attachments are byte copies of source content and, like the kept body files, are exempt from the file contract's generated-file rules (see the exemption added to that ADR).
2. **Chunked copy.** The bytes are copied from the source stream to the staging directory in fixed-size chunks (256 KiB; a tunable, not part of the contract), hashing as they are written, one attachment at a time. No whole attachment is held in memory. The model carries a descriptor per attachment (method, content ID, the length the container declares) and never bytes; the bytes are read at staging time through a reader the stager does not know the source of.
3. **Integrity.** The size and SHA-256 recorded are those of the bytes written. The number of bytes copied must equal the length the container declares; otherwise the attachment is `failed`.
4. **A source failure is a recorded failure, not a failure of the run.** If an attachment's data stream is missing, cannot be opened, fails to read part-way, or has a length other than the declared one, the partial staged file is removed, nothing is added to the manifest, no staged number is used, the attachment is recorded as `failed` with a reason, and the export continues. A failure to write to staging stops the run and triggers the cleanup of the output-posture ADR.
5. **Not extracted by design, with a reason, never silent.** Embedded messages (until M4f-2), OLE objects, by-reference attachments (methods 2 to 4), method 0 (none), an unrecognized method, an unreadable method, and an embedded-message attachment whose nested message could not be read are recorded as `not_extracted`.
6. **Zero-byte attachments** are written as empty files and recorded with `bytes` 0.
7. **Staging order within a message:** its attachments first, then `metadata.json` (which records each size and SHA-256, known only after the copy), then `message.md` and the kept body files.
8. **The record** (draft schema, frozen at M4g): `metadata.json` gains an `attachments` array, in storage order, one object per attachment with these fields in this order: `name` (`original` and `adjustments`, as for directory names), `method` (a label: `by_value`, `by_reference`, `by_reference_resolve`, `by_reference_only`, `embedded_message`, `ole`, `none`, `unrecognized`, `unreadable`), `method_code` (the stored integer, or null), `content_id`, `status` (`extracted`, `not_extracted`, `failed`), `reason`, `file`, `bytes`, `sha256` (the last three null unless extracted). Reasons, a draft vocabulary: not extracted: `embedded_message`, `ole_object`, `by_reference`, `method_none`, `method_unrecognized`, `method_unreadable`, `embedded_message_unreadable`; failed: `data_stream_missing`, `data_read_failed`, `data_length_mismatch`. `attachments_not_extracted` counts the records whose status is not `extracted`, and the message's status reason `attachments_not_extracted` is listed exactly when that count is above zero. The root `folder.json` counts gain `attachments_extracted`, and its `attachments_not_extracted` is the number of planned attachment files not written (it includes failed attachments and those inside embedded messages). The export report gains `export_attachments_extracted`, `export_attachment_bytes_extracted`, and `export_attachments_failed`, and `export_attachments_not_extracted` has the root's meaning.
9. **The attachment `sha256` is a per-file integrity hash.** It is not the message content hash of the [identity ADR](message-and-folder-identity-and-provenance.md) (point 5, Q11); whether that definition includes attachment hashes is decided at M4g.
10. **The diagnostics follow the same principle.** The zero-byte check reads each by-value stream through in chunks instead of holding it, and the directory entry's length is used only as the declared length for the export: using it alone for the zero-byte check would stop the diagnostic noticing a stream whose sector chain is broken.

### Consequences

* Good, because memory is a constant instead of the largest attachment in a corpus whose sizes are not known; the writer's peak does not grow with attachment size.
* Good, because the hash is computed in the same pass that writes the file, and the recorded size and hash are those of what is on disk.
* Good, because a damaged or missing attachment costs one record, not the archive, and every attachment has a record whatever happened to it.
* Good, because the stager takes any reader: a PST attachment reader (M4i) can feed it without changing the stager or the model.
* Bad, because `Stager::add_stream` is a second write path beside `Stager::add`; it is guarded by a test that both give the same file, size, hash, and tree hash for the same bytes.
* Bad, because a partly written file has to be cleaned up when a read fails.
* Bad, because a repeat export copies every attachment into staging before finding it identical (the shortcut, option E of the output-posture plan, is not built), and attachments dominate an archive's bytes, so repeat exports of large archives are slow.
* Bad, because the declared-length check cannot fire with the current source (the container reads a stream to its declared length or fails); it is a guard for other sources and has been exercised only by a stand-in reader.
* Bad, because the record and its vocabulary are drafts until M4g, and the flat staging directory and the manifest are untried at tens of thousands of attachments.

### Confirmation

* **Synthetic tests (M4f-1, 332 tests pass, reported by the owner):** the stager's chunked copy gives the same file, size, hash, and tree hash as a whole-file add for a payload that crosses chunk boundaries; it survives short reads and an interrupted read, stages an empty source as an empty file, and, for a source that fails or has the wrong length, leaves no staged file and uses no staged number; attachment descriptors and data streams are read in storage order; the archive record and method labels; and, through a real export, three by-value attachments (more than two chunks, a few bytes, none) written with the recorded size and SHA-256, duplicate and missing names landing under their own planned names (and a second export writing nothing), and a message mixing OLE, by-reference, a by-value attachment with no data stream, and one that extracts.
* **Memory gate (synthetic, the owner's measurement, 2026-10-18):** 3 messages, each with one by-value attachment of 1, 50, or 200 MB: peak working set 28 MB and peak paged memory 25 MB at every size, exit code 0, 16 files written, 0 read-back mismatches.
* **Manual check:** the SHA-256 in `metadata.json` for the 200 MB attachment equals `Get-FileHash` of the exported file.
* **Not yet confirmed:** an export of the real corpus with attachments (no corpus tree hash for `v0.1.34` yet, and no check that every corpus attachment appears in the archive or in diagnostics); the SHA-256 comparison of each extracted attachment with `msg_parser`'s payload (the M4f gate, M4f-2); `verify-split.ps1` after the rewrite of the zero-byte check; adversarial attachment names beyond duplicate and missing ones (reserved names, 255-character names, case-only differences, control characters); an attachment of gigabyte size (the largest streamed was 200 MB); thousands of attachments in one archive; `cargo fmt --check`, `clippy -D warnings`, and CI for the tag (not reported); attachments from a PST (M4i).

## Pros and Cons of the Options

### A. Chunked copy with hashing as written

* Good, because memory is constant and the pass is single.
* Good, because the rest of the staging code already streams (`hash_file`, the preflight, the read-back).
* Bad, because of the second write path and the partial-file cleanup (Consequences).

### B. Read one attachment whole, then stage it

* Good, because it reuses `Stager::add` unchanged and a read failure leaves nothing behind.
* Bad, because the peak is the largest attachment in a corpus whose sizes are not known (a whole-stream read with doubling growth needs roughly one and a half to two times the attachment transiently; an estimate, not measured), and it would have to be reworked later for the PST.
* Neutral, because it needs the model to carry descriptors and the bytes to be read at staging time just as A does, so it is not simpler where it matters.

### C. Read all of a message's attachments into the model

* Good, because rendering would see everything at once.
* Bad, because memory grows with the sum of a message's attachments, and the model would hold bytes it never needs together.

### D. Copy straight to the final path

* Good, because no staging copy.
* Bad, because it writes before the consent decision into the archive proper and breaks the staging, consent, and read-back rules of the output-posture ADR.

### Failure: B. abort the export

* Good, because it is simple and loud.
* Bad, because one damaged attachment in a multi-gigabyte store would stop everything, and the posture ADR wants a partly bad source to fail alone.

### Failure: C. skip silently

* Bad, because it violates rule 6 of the file contract: loss must be recorded.

### Record: B. only extracted attachments plus a count

* Good, because a smaller `metadata.json`.
* Bad, because the reason an attachment is absent would live nowhere.

### Record: C. a separate `attachments.json`

* Good, because `metadata.json` stays small.
* Bad, because it adds a file to every message with attachments and separates the record from the message it describes, which the archive ADR kept together.

## More Information

* Plan and stage: [`docs/plans/m4-plan.md`](../docs/plans/m4-plan.md) (M4f; "What changed from v4.22"). Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md) (M4f-1 section).
* Related: [Deterministic Markdown archive](deterministic-markdown-archive.md), [Export naming, collisions, and path budgets](export-naming-collisions-and-path-budgets.md), [Export output posture](export-output-posture-consent-and-ownership.md), [Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md), [Body selection, original bodies, and formatting loss](body-selection-original-bodies-and-formatting-loss.md), [Message and folder identity](message-and-folder-identity-and-provenance.md).
* Follow-ups: embedded messages as nested message folders at the depth cap, `cid:` rewriting in `message.md`, the `msg_parser` payload comparison, and the remaining adversarial fixtures (M4f-2); attachment bytes from a PST, which `outlook-pst` 1.2.0 does not expose (M4i); the closed vocabulary and the first numbered schema (M4g); the repeat-export shortcut (option E) and sharding of the staging directory, if they prove needed.
* Revisit when: embedded messages need a record shape this one cannot carry, the PST reader can supply attachment bytes, the closed vocabulary is defined (M4g), or repeat exports of large archives prove too slow.
