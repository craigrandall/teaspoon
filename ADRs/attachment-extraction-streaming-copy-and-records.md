---
status: "proposed"
date: 2026-10-18
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Attachment extraction: chunked copy, per-attachment records, and failure handling

*Status history: proposed 2026-10-18. The owner accepted the choices below as recommended on 2026-10-18, before M4f-1 was written; this text records them and counts as accepted when the owner commits it. The behavior is implemented for by-value attachments of top-level messages (M4f-1, tag `v0.1.34`) and was tested on synthetic input and measured with a synthetic memory gate. Amended 2026-10-20 (M4f-2a and M4f-2c, release `v0.1.35`): the owner accepted, as recommended, decisions 11 to 14 below (embedded messages as nested message folders, the record of one at the depth cap, the counters, and the byte-level differential), which are implemented and were run on the real corpus (see Confirmation); this text counts as accepted when the owner commits it. Also approved on 2026-10-20 and to be written up with M4f-2b, which is not built: marking inline attachments (an attachment is `inline` when its content ID is referenced by a `cid:` link in the body that `message.md` was made from) and rewriting `cid:` references in `message.md` to the extracted files. Not decided here: attachments read from a PST (M4i), and the closed status-reason vocabulary and the first numbered `schema_version` (M4g).*

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

1. **What is written.** Each by-value attachment (`PidTagAttachMethod` 1) of a message (a top-level message, or an embedded message nested within the depth cap, decision 11) is written byte for byte as `attachments/<planned name>` in its message folder, under the name the plan assigned. Extracted attachments are byte copies of source content and, like the kept body files, are exempt from the file contract's generated-file rules (see the exemption added to that ADR).
2. **Chunked copy.** The bytes are copied from the source stream to the staging directory in fixed-size chunks (256 KiB; a tunable, not part of the contract), hashing as they are written, one attachment at a time. No whole attachment is held in memory. The model carries a descriptor per attachment (method, content ID, the length the container declares) and never bytes; the bytes are read at staging time through a reader the stager does not know the source of.
3. **Integrity.** The size and SHA-256 recorded are those of the bytes written. The number of bytes copied must equal the length the container declares; otherwise the attachment is `failed`.
4. **A source failure is a recorded failure, not a failure of the run.** If an attachment's data stream is missing, cannot be opened, fails to read part-way, or has a length other than the declared one, the partial staged file is removed, nothing is added to the manifest, no staged number is used, the attachment is recorded as `failed` with a reason, and the export continues. A failure to write to staging stops the run and triggers the cleanup of the output-posture ADR.
5. **Not extracted by design, with a reason, never silent.** Embedded messages nested past the depth cap (decision 12), OLE objects, by-reference attachments (methods 2 to 4), method 0 (none), an unrecognized method, an unreadable method, and an embedded-message attachment whose nested message could not be read are recorded as `not_extracted`.
6. **Zero-byte attachments** are written as empty files and recorded with `bytes` 0.
7. **Staging order within a message:** its attachments first, then `metadata.json` (which records each size and SHA-256, known only after the copy), then `message.md` and the kept body files.
8. **The record** (draft schema, frozen at M4g): `metadata.json` gains an `attachments` array, in storage order, one object per attachment with these fields in this order: `name` (`original` and `adjustments`, as for directory names), `method` (a label: `by_value`, `by_reference`, `by_reference_resolve`, `by_reference_only`, `embedded_message`, `ole`, `none`, `unrecognized`, `unreadable`), `method_code` (the stored integer, or null), `content_id`, `status` (`extracted`, `not_extracted`, `failed`), `reason`, `file`, `bytes`, `sha256` (the last three null unless extracted). Reasons, a draft vocabulary: not extracted: `embedded_depth_capped` (new with M4f-2a; it replaces `embedded_message`, which is no longer written), `ole_object`, `by_reference`, `method_none`, `method_unrecognized`, `method_unreadable`, `embedded_message_unreadable`; failed: `data_stream_missing`, `data_read_failed`, `data_length_mismatch`. `attachments_not_extracted` counts the records whose status is not `extracted`, and the message's status reason `attachments_not_extracted` is listed exactly when that count is above zero. The root `folder.json` counts gain `attachments_extracted`, and its `attachments_not_extracted` is the number of planned attachment files not written (it includes failed attachments and those inside embedded messages that were not written). The export report gains `export_attachments_extracted`, `export_attachment_bytes_extracted`, and `export_attachments_failed`, and `export_attachments_not_extracted` has the root's meaning.
9. **The attachment `sha256` is a per-file integrity hash.** It is not the message content hash of the [identity ADR](message-and-folder-identity-and-provenance.md) (point 5, Q11); whether that definition includes attachment hashes is decided at M4g.
10. **The diagnostics follow the same principle.** The zero-byte check reads each by-value stream through in chunks instead of holding it, and the directory entry's length is used only as the declared length for the export: using it alone for the zero-byte check would stop the diagnostic noticing a stream whose sector chain is broken.

11. **Embedded messages are nested message folders (M4f-2a).** An attachment whose method is "embedded message" (5) and whose storage opens as a message is written as a message directory at the directory the planner gave it, `<message>/attachments/<name>/`, with `message.md`, `metadata.json`, the kept body files, and its own `attachments/` folder (which may hold further embedded messages), by the same function that writes a top-level message, one level in memory at a time; attachment bytes stream as before. Its `metadata.json` has the message schema with `kind` `embedded_message` and `source_file` the outer `.msg`'s relative name; its position is its folder path. The parent's attachment record has `method` `embedded_message`, `status` `extracted`, and `file` the directory name, with no `bytes` or `sha256`.
12. **The depth cap.** The planner's cap (three levels written in full) holds. A message at the next level gets a directory and only a `metadata.json`: `kind` `embedded_message`, `source_file`, `subject`, `directory` (its name record, whose adjustments include `depth_capped`), `attachments_not_extracted` (its direct attachments), `status` `partial`, and the status reasons `["embedded_depth_capped"]`; nothing is planned or written below it, and the parent's record says `not_extracted` with the reason `embedded_depth_capped`. An embedded-method attachment whose storage cannot be opened as a message stays a file attachment recorded `embedded_message_unreadable`, as before.
13. **Counters.** `export_embedded_messages_extracted` counts the nested directories written in full; `export_embedded_messages_not_extracted` is the planned embedded messages minus those, computed from the plan (so planned = extracted + not extracted holds, and a message below an unwritten one is counted); `export_embedded_body_loss_total` sums the formatting-loss counts over the bodies of embedded messages written. The existing body counters and `export_message_directories` still count top-level messages only, so their meaning and the identities among them do not change. `export_attachments_extracted`, the byte total, and the root counts include attachments inside embedded messages that were written.
14. **A byte-level differential for the by-value copy (M4f-2c).** `--verify-attachments` hashes each top-level by-value attachment through the export's own reader and compares its SHA-256 and length with `msg_parser`'s payload for the attachment at the same position (an order-free comparison is printed beside it), counts only, with the gate `attachment_payload_gate_violations`. It does not open embedded messages, as `msg_parser` does not; it is a separate flag so that the `--verify` output stays unchanged.

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
* Good (M4f-2a), because an embedded message is the same kind of thing as a message and is written by the same code, so it gets the envelope, the body, and the loss record without a second implementation, and its attachments reuse the chunked copy.
* Bad (M4f-2a), because the nested tree makes paths longer (the planner already reserves for it) and the counters of top-level and embedded messages are separate, which a reader of the report has to know; the depth cap hides what is below it (recorded as a count, not as content); and the attachments inside embedded messages have no independent byte-level check.

### Confirmation

* **Synthetic tests (M4f-1, 332 tests pass, reported by the owner):** the stager's chunked copy gives the same file, size, hash, and tree hash as a whole-file add for a payload that crosses chunk boundaries; it survives short reads and an interrupted read, stages an empty source as an empty file, and, for a source that fails or has the wrong length, leaves no staged file and uses no staged number; attachment descriptors and data streams are read in storage order; the archive record and method labels; and, through a real export, three by-value attachments (more than two chunks, a few bytes, none) written with the recorded size and SHA-256, duplicate and missing names landing under their own planned names (and a second export writing nothing), and a message mixing OLE, by-reference, a by-value attachment with no data stream, and one that extracts.
* **Memory gate (synthetic, the owner's measurement, 2026-10-18):** 3 messages, each with one by-value attachment of 1, 50, or 200 MB: peak working set 28 MB and peak paged memory 25 MB at every size, exit code 0, 16 files written, 0 read-back mismatches.
* **Manual check:** the SHA-256 in `metadata.json` for the 200 MB attachment equals `Get-FileHash` of the exported file.
* **Real corpus (M4f-2c and M4f-2a, 2026-10-20, the owner's runs; `Cargo.toml` at 0.1.34):** `--verify-attachments --recursive` over 34 files: 35 attachments on both sides, 30 by-value pairs, 30 SHA-256 and length matches, 0 mismatches, 0 missing or failed streams, 0 method disagreements, 6,282,892 bytes matched, the order-free comparison 34 of 34, `attachment_payload_gate_violations=0`. Export of the corpus: 3 of 3 embedded messages written (0 not extracted), 32 attachment files extracted and 2 not extracted (34 planned), 0 failed, 217 files written (171 at `v0.1.33`), 0 read-back mismatches, a repeat export identical (217 unchanged, 0 written), the top-level body counters unchanged, `export_tree_sha256=845b21aeaec075099c7064d5f4da1372ed7084e452774f2b0214d86c742831e4` (the hash at the tag, with `Cargo.toml` at 0.1.35, is `e27f0d87bf1619a31d091916f4dd2a17bcb778c9c14d780e09bac2a1246e38cd`). `verify-split.ps1` IDENTICAL for the default, `--verify`, and PST outputs. Synthetic tests cover one level of nesting, duplicate subjects, the depth cap, an unreadable embedded storage, and the reader directly.
* **Not yet confirmed:** the depth cap and the unreadable embedded storage on real data (the corpus's 3 embedded messages are all written in full); an owner review of a nested message directory; the 2 extracted files beyond the 30 top-level by-value attachments (consistent with empty by-value attachments inside embedded messages, not itemized); a regression check against `v0.1.34` on a corpus subset without embedded messages and a re-run of `--verify-attachments` with this build (part of the gate; their output was not supplied); adversarial attachment names beyond duplicate and missing ones (reserved names, 255-character names, case-only differences, control characters); an attachment of gigabyte size (the largest streamed was 200 MB); thousands of attachments in one archive; `cargo fmt --check` and CI for the tag (not reported); attachments from a PST (M4i).

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

### Embedded messages (M4f-2a): options considered

* **A. Nested message folders written by the same function as a top-level message, to the planner's depth cap (chosen).** Good, because it reuses everything, keeps the archive's one shape (a message is a folder), and lets the planner's budget and naming rules apply unchanged. Bad, because paths grow and a message past the cap is only recorded.
* B. Write the embedded message as a single file (for example a `.msg`). Good, because one file per embedded message. Bad, because the tool has no `.msg` writer, a re-serialization would not be a byte copy, and the content would not be in the Markdown archive.
* C. Keep recording embedded messages as not extracted. Good, because nothing to build. Bad, because the content (often a forwarded message) is lost from the archive, which the contract's rule 6 allows only if recorded, and the owner wants it mined.
* D. Flatten embedded messages into the parent's `message.md`. Good, because one file. Bad, because it mixes two messages' envelopes and bodies in one projection, and it cannot carry their attachments or their own records.

## More Information

* Plan and stage: [`docs/plans/m4-plan.md`](../docs/plans/m4-plan.md) (M4f; "What changed from v4.22" and "What changed from v4.23"). Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md) (M4f-1, M4f-2c, and M4f-2a sections).
* Related: [Deterministic Markdown archive](deterministic-markdown-archive.md), [Export naming, collisions, and path budgets](export-naming-collisions-and-path-budgets.md), [Export output posture](export-output-posture-consent-and-ownership.md), [Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md), [Body selection, original bodies, and formatting loss](body-selection-original-bodies-and-formatting-loss.md), [Message and folder identity](message-and-folder-identity-and-provenance.md).
* Follow-ups: `cid:` rewriting in `message.md` and inline marking, and the remaining adversarial fixtures (M4f-2b and after); attachment bytes from a PST, which `outlook-pst` 1.2.0 does not expose (M4i); the closed vocabulary and the first numbered schema (M4g); the repeat-export shortcut (option E) and sharding of the staging directory, if they prove needed.
* Revisit when: the nested-message record needs more than this (for example for embedded objects that are not messages), the PST reader can supply attachment bytes, the closed vocabulary is defined (M4g), or repeat exports of large archives prove too slow.
