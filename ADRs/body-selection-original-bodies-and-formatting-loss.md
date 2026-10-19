---
status: "accepted"
date: 2026-10-12
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Body selection, original bodies, and formatting-loss recording

*Status history: proposed and accepted 2026-10-12 (the owner accepted the seven recommendations, then this ADR as written, including decision 8). Amended 2026-10-13 (decision 4): the loss vocabulary gains `html_conversion_failed`, found necessary while writing the pure body module, because a conversion that fails must be recorded and cannot be silent; nothing else changed. Amended 2026-10-14 (decisions 4 and 9): the vocabulary gains `rtf_decompression_failed`, found necessary while wiring the body into the export, because an RTF property that exists but cannot be decompressed is a form the message had and must not vanish without a record; the status rule for unconverted bodies is stated; and the choices M4e-2b had to make where this ADR was silent (the layout of the `body` object, what "exists" means, which native-HTML property type is read, the layout of `message.md`, and the export report keys) are recorded as decision 9. It completes [HTML-to-Markdown converter and preprocessing](html-to-markdown-converter-and-preprocessing.md), which decided the converter, its pin, the preprocessing, and the policies for layout tables, hidden text, and link destinations; it amends one of them, the preprocessing list (decision 8). Amended 2026-10-18 (M4f-1; proposed, counts as accepted when the owner commits it): the kept-file exemption of decision 2 is extended to extracted attachments by the [file contract ADR](archive-file-contract-and-schema-evolution.md), and decision 3 is clarified: the attachment files exist since M4f-1, but nothing rewrites the `cid:` references yet, and the rewrite moves to M4f-2. Not decided here: attachment extraction (decided in the [attachment ADR](attachment-extraction-streaming-copy-and-records.md)) and the rewriting of `cid:` references to files (M4f-2), the closed status-reason vocabulary and the first numbered `schema_version` (M4g), and bodies read from a PST (M4i).*

## Context and Problem Statement

A message can carry its body in up to three forms: plain text (`PidTagBody`), HTML (`PidTagHtml`), and compressed RTF (`PidTagRtfCompressed`), which may encapsulate HTML. In the corpus, 27 of 29 top-level `.msg` files carry their HTML only inside the RTF (the in-house de-encapsulation, M4e-1, recovers it) and none carries native HTML. Before M4e-2b, `message.md` held only the plain-text body in a code fence (M4c, explicitly provisional). With the converter decided, four questions remain, and each is expensive to reverse because it fixes what an archive contains: which form becomes the body in `message.md`; which original forms the archive keeps, under what names; how a conversion that loses something says so; and how the output is verified. A fifth, smaller one came out of the by-eye review: some newsletter templates put about 600 invisible characters of padding into every message.

## Decision Drivers

- Nothing the message contained may be lost without a record: the original forms stay recoverable.
- `message.md` must be readable, and the selection deterministic and explainable.
- Loss is explicit, named from a closed list, and counted.
- Archives must stay within the path budget and the file-contract rules ([Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md)); no new naming problems.
- The result must be checkable, cheaply and without reading content aloud (counts only).
- A better converter later must not require the original message again.

## Considered Options

For original bodies:

* **A. Keep every original form that exists, as flat files in the message folder (chosen).**
* B. Keep only the form that was selected.
* C. Keep nothing; the Markdown is the archive.
* D. Keep the originals in an `original/` subfolder.

For `cid:` references: **A. leave them as written and count them (chosen)**; B. rewrite them to `attachments/` paths now; C. drop the images.

For invisible padding: **A. collapse long runs in `message.md` only (chosen)**; B. keep everything; C. drop all hidden elements.

## Decision Outcome

Chosen options as marked, because they lose nothing that cannot be recovered from a kept original, keep `message.md` readable, and add no naming or budget problem.

**Binding decisions:**

1. **Body selection.** For each message, in this order: (a) native HTML, if present and non-empty; (b) otherwise the HTML recovered from the RTF; (c) otherwise the plain-text body; (d) otherwise no body. HTML sources are converted as the converter ADR decides. A plain-text source is written verbatim in a code fence longer than any backtick run in it (the M4c rendering, now final). An RTF that carries no encapsulated HTML is not converted (there is no RTF converter), the plain-text body is used if there is one, and the reason is recorded. The source used is recorded in `metadata.json` as one of `native_html`, `rtf_html`, `plain_text`, `none`.
2. **Original bodies are kept verbatim as flat files in the message folder:** `body.html`, `body.rtf`, and `body.txt`, each written only if that form exists in the message, whether or not it was the selected one. `body.html` holds the native HTML bytes as stored (the declared code page, `PidTagInternetCodepage`, is recorded in metadata) or, when recovered from RTF, the recovered UTF-8 text (whose `charset=` declaration is stale; metadata records that it was declared). `body.rtf` holds the decompressed RTF bytes. `body.txt` holds the plain-text body as UTF-8. Preprocessing, conversion, and the padding rule below apply to copies in memory; the kept files are never altered. Each kept file has its byte length and SHA-256 in metadata. These files are byte copies of source content and are exempt from the file contract's UTF-8-without-BOM and LF rules, which govern files the tool generates; metadata says so. Tool-written files follow the overwrite and consent rules of the output-posture ADR like any other. **No budget or naming change:** each name is shorter than `metadata.json`, which already sets the worst-case child reservation, and a message directory has no user-named children, so nothing needs a reserved name.
3. **`cid:` references stay as written** in `message.md`. The number of `cid:` references and of distinct ones is recorded in `metadata.json`. Rewriting them to relative `attachments/` paths belongs to M4f-2: the attachment files exist since M4f-1, but nothing rewrites the references yet, and the rewrite will use each attachment's planned name, which the plan knows before the attachment's bytes are copied.
4. **Loss is recorded in a `body` object in `metadata.json`** (draft schema `0.1-draft`, frozen at M4g under the evolution policy): the source; for HTML sources, the converter's name and version (`htmd`, `0.5.4`); the preprocessing steps applied, from a closed list (`comments`, `style`, `script`, `title`, `downlevel_markers`, `office_paragraph_tags`, `invisible_padding`); the kept original files; and `loss`, a list of `{reason, count}` from a closed vocabulary that starts as: `tables_flattened` (table rows in the HTML that have no Markdown table row), `rtf_not_converted` (an RTF existed that could not be used), `undecodable_bytes_replaced` (bytes the de-encapsulation could not decode, replaced by U+FFFD), `invisible_padding_removed` (the number of runs removed), `html_conversion_failed` (the converter failed, panicked, or could not start; the plain-text body, if any, is used instead, and the kept original bodies are unaffected), and, added 2026-10-14, `rtf_decompression_failed` (a non-empty `PidTagRtfCompressed` existed and could not be decompressed; it is recorded whichever source was selected, no `body.rtf` is written because there are no decompressed bytes to keep, and the other forms are used as usual). The vocabulary's order is the order listed here; `loss` lists only reasons with a count above zero, in that order. A new reason is a schema change. **Status rule (stated 2026-10-14):** the M4c status reason `formatted_bodies_not_converted` is recorded for a message exactly when an HTML or RTF form existed (including an RTF that could not be decompressed) and the body used is `plain_text` or `none`: a conversion that failed, an RTF with no encapsulated HTML, or an RTF that could not be decompressed. It is not recorded for a message whose body was converted. How reasons roll up into the message's `partial` status and its reasons is otherwise decided with the status vocabulary at M4g.
5. **Invisible padding is collapsed in `message.md` only.** After conversion, a run of at least 8 characters from {U+034F, U+00AD, U+200B, U+200C, U+200D, U+2060, U+FEFF}, with any spaces and no-break spaces between and around them, is removed; a line left empty is removed, and consecutive blank lines left behind collapse to one. Shorter runs, and the direction marks U+200E and U+200F, stay. Each run removed is counted as `invisible_padding_removed`. The threshold is conservative: the runs seen are 200 characters long, and emoji joiner sequences are never 8 invisible characters in a row.
6. **A content-preservation check** is part of the product: a pure function and a counts-only corpus mode (`tsp --verify-bodies <input>`) that compare, in order, the visible words of the HTML (comments, `head`, `style`, and `script` excluded; inline tags with no Markdown delimiter glued) with the words of the Markdown (image alt text, link destinations, ordered-list marker numerals, and removed padding excluded). It prints counts only: words in the HTML, words in the Markdown, words matched in order, and the number of differences. The documented exclusions are the only ones. The gate is no difference over the corpus. It is implemented in-house (no new dependency) and unit-tested.
7. **Work items, not decisions:** `htmd` becomes a normal dependency pinned `=0.5.4`; `preprocess_office_html` moves out of the bake-off module with its tests; golden tests cover each source and the tricky cases (empty body, plain text only, RTF without encapsulated HTML, an unterminated style element, padding runs, `cid:` images).

8. **The preprocessing also removes `<title>` elements** (with their text, in the same single pass as comments, `style`, and `script`), which amends the converter ADR's preprocessing list; that ADR records the amendment. Evidence: `htmd` writes a non-empty `<title>` as the first line of the Markdown. The owner checked six messages: in 001, 008, 009, 023, and 024 the first line of the Markdown equals the `<title>` text (11, 3, 4, 8, and 3 words), accounting for 28 of the 37 extra words the harness could not otherwise explain, and 029 has no title. The title is not body text and repeats the subject, which `message.md` already shows as its heading. The harness uses the same preprocessing function, so its fitness function stays aligned; the expected effect of a re-run is about 28 fewer extra words, nothing else changed, and it is to be confirmed. Only `<title>` is removed, not the whole `<head>`, because an unterminated `<head>` would drop the body, and an unterminated `<title>` is by the HTML rules title text to the end of the document.

9. **Choices made while wiring the body into the export (M4e-2b), where decisions 1 to 8 were silent.** Draft schema, frozen at M4g.
   * **"Exists" means present and not zero-length.** A form that is absent or zero-length gets no kept file and no mention. A whitespace-only plain body exists: `body.txt` keeps it, though it is not selected or shown. A whitespace-only native HTML exists and is kept as `body.html`, though selection falls through to the RTF.
   * **The `body` object's fields**, in this order: `source`; `converter` (`{name, version}`, or null when `message.md` holds no converted Markdown); `preprocessing`; `internet_codepage` (`PidTagInternetCodepage` as stored when it is a positive integer, else null; recorded whether or not native HTML exists); `originals`, a list in the fixed order `body.html`, `body.rtf`, `body.txt` of `{file, bytes, sha256, note}`; `cid_references` (`{total, distinct}`, counted over the link destinations in the converted Markdown, case-insensitively for the distinct count); `loss`; and `kept_files_exempt_from_file_rules` (always true). `note` is `as_stored` for native `body.html`, `recovered_from_rtf_charset_declaration_stale` for `body.html` recovered from the RTF, and null otherwise. The earlier draft's presence flags (`html_native`, `html_via_rtf`, `rtf`, `plain_text`, `markdown`) are gone: `source` and `originals` carry the same information.
   * **`body.html` when both exist.** When a message has native HTML, `body.html` holds it, whichever source was selected; the HTML recovered from the RTF is then not kept separately, because `body.rtf` already preserves it and any converter can re-derive it.
   * **Native HTML is read as `PT_BINARY` only** (`0x1013` with type `0x0102`, as the verified body flags have always read it). The string-typed variant of the same property ID is not read; if a producer is found that writes it, reading it is a follow-up and the loss would otherwise be silent, so this is also listed under "Not yet confirmed".
   * **The layout of `message.md`:** the heading, then the envelope list, then one blank line, then the body. An HTML source's Markdown is written as converted, with CRLF and bare CR normalized to LF, leading blank lines and trailing whitespace removed, and one final newline. A plain-text source keeps the code fence of decision 1. There is no separator and no loss banner: loss lives in `metadata.json` only. A body whose Markdown is blank leaves the heading and envelope alone, with no loss reason, because the vocabulary is closed.
   * **The export report** (counts only, every key always printed) keeps `export_bodies_plain_present`, `export_bodies_plain_empty`, and `export_bodies_plain_absent` (a zero-length or whitespace-only plain body is "empty") and adds `export_body_source_<native_html|rtf_html|plain_text|none>`, `export_body_files_kept_<html|rtf|txt>`, `export_body_loss_<reason>` (the sum of the counts, one key per reason in vocabulary order), and `export_body_cid_references_total`.

### Consequences

* Good, because every form the message had is in the archive, so a better converter can re-derive `message.md` from `body.html` and `body.rtf`, and nothing is lost to a conversion bug.
* Good, because loss has names and counts in metadata, and the content check is reproducible and content-free.
* Good, because `message.md` loses the document title and the invisible padding (about 1.5 KB per message in the shared newsletter template) while the original keeps it.
* Bad, because the archive grows: the HTML (40 to 65 KB for the larger corpus messages) and the RTF, which is larger than the HTML it carries, are stored beside the Markdown.
* Bad, because `body.rtf` and a non-UTF-8 `body.html` are not text the tool can read back as UTF-8, and they are exempt from the generated-file rules.
* Bad, because the export holds every kept body in memory until the archive is written, which is fine for the corpus and not for multi-gigabyte mail stores; the M4h plan pulls streaming forward for that reason.
* Bad, because `cid:` images do not display from `message.md` until M4f-2.
* Bad, because the loss vocabulary and the `body` object are drafts until M4g, and the padding threshold and the exclusion list in the check are judgments that real mail may challenge.

### Confirmation

* **Evidence so far:** the by-eye review of four pairs (007, 015, 022, 027), recorded in the converter ADR and `m4-results.md`: the word sequence of the Markdown equals the HTML's, in order, with the exclusions above, in all four; the padding runs (200 U+034F, 200 U+00AD, about 200 no-break spaces) in 007 and 027; the `cid:` references in 015 and 007; the one U+FFFD (015) that is already in the recovered HTML.
* **Built and verified by the automated gate (M4e-2b, v0.1.31, 2026-10-14):** the export writes the converted Markdown, the kept original files, and the `body` object; unit and golden tests cover each source, the failure paths, and determinism. Corpus run (34 messages): 31 messages use the HTML recovered from the RTF, 3 fall back to the plain text, none has native HTML, none failed conversion or RTF decompression; 99 kept files, all matching their recorded size and SHA-256; read-back mismatches 0; the same tree hash in two directories; a repeat export writes nothing. Recorded loss: 237 table rows flattened, 3 `rtf_not_converted`, 1 undecodable byte, 1 invisible-padding run, 26 `cid:` references. The owner review of exported messages is pending (`m4-results.md`).
* **Built and run (M4e-2c, `v0.1.32`, 2026-10-15):** over the 34-message corpus (31 HTML bodies) the check found 26,145 HTML words, 26,385 Markdown words, 26,145 matched in order, 0 missing, 0 extra, 240 syntax words excused, 0 unresolved entities. It differs from the wording of decision 6 in three ways, each stated so it can be overruled: (a) the emphasis and code characters (asterisk, underscore, tilde, backtick) and the invisible padding characters are removed from every word on both sides, where the decision names only removed padding; (b) Markdown syntax words (runs of hash, greater-than, hyphen, plus, vertical bar, colon, or equals characters, and ordered-list numerals) are excused only where they sit against a different HTML word, where the decision excludes list numerals unconditionally, so a literal hyphen or numeral that the HTML really had is still reported; (c) the comparison is a greedy in-order resynchronization (window of 64 words), not a minimal edit script, so zero differences is exact and a non-zero count is approximate. It starts from the HTML after the preprocessing, so it does not cover text the preprocessing removes. Gate as first planned: `--verify-bodies` over the 29-message corpus with no difference, and its exclusions listed; the exported tree reconciled with the counts (every message has a `body` object, every kept file's hash matches its metadata); two exports byte-identical; owner review of a sample.
* **Title evidence (done 2026-10-12):** in 001, 008, 009, 023, and 024 the first line of the Markdown equals the `<title>` text; 029 has no title; 007, 015, 022, and 027 have none or an empty one. The Cyrillic words in 023's extra-word listing are image alt text that is entity-encoded in the HTML (`alt` attributes with numeric entities were found in 023 and 008), so the listing script missed them; one extra word in 029 is unexplained.
* **Not yet confirmed:** the native-HTML path on real data (the corpus has none; synthetic tests cover it); a string-typed `PidTagBodyHtml` from other producers (not read); an RTF without encapsulated HTML (three corpus messages, observed: they fall back to the plain text and record `rtf_not_converted`; the RTF itself is not converted); `rtf_decompression_failed` on real data (zero in the corpus, observed); bodies from other producers; the padding threshold on mail other than this template, and why only one padding run was removed in the corpus run where the evidence above suggested at least two (unchecked); whether `tables_flattened` should count tables or rows (237 rows in the corpus run).

## Pros and Cons of the Options

### A. Keep every original form that exists, flat

* Good, because it is lossless, simple, and changes no path budget.
* Bad, because of the size and the exemption noted above.

### B. Keep only the selected form

* Good, because it is smaller.
* Bad, because the other forms are message content, and a plain-text body that differs from the HTML would be lost.

### C. Keep nothing

* Good, because it is smallest and simplest.
* Bad, because every converter defect and every loss would be permanent.

### D. An `original/` subfolder

* Good, because the message folder stays short.
* Bad, because it adds a directory and 9 units to every kept path for no gain; the fixed names cannot collide.

### `cid:` references: B. rewrite now

* Good, because images would display sooner.
* Bad, because the files do not exist until M4f, so the links would point at nothing.

### `cid:` references: C. drop images

* Bad, because it loses content and breaks the converter ADR's driver that images survive.

### Padding: B. keep everything

* Good, because it changes nothing.
* Bad, because roughly 1.5 KB of invisible characters per message clutter the Markdown and searches.

### Padding: C. drop all hidden elements

* Bad, because hidden preheader text is real text; the converter ADR keeps it, and dropping cannot be undone from the Markdown.

## More Information

* Plan and stage: [`docs/plans/m4-plan.md`](../docs/plans/m4-plan.md) (M4e-2). Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md).
* Related: [HTML-to-Markdown converter and preprocessing](html-to-markdown-converter-and-preprocessing.md), [Deterministic Markdown archive](deterministic-markdown-archive.md), [Export naming, collisions, and path budgets](export-naming-collisions-and-path-budgets.md), [Export output posture](export-output-posture-consent-and-ownership.md), [Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md).
* Follow-ups: rewriting `cid:` references (M4f-2; attachment extraction itself is decided in the [attachment ADR](attachment-extraction-streaming-copy-and-records.md)); the status-reason vocabulary and the first numbered schema (M4g); streaming export for large stores (M4h, pulled forward); PST bodies (M4i).
* Revisit when: the content check reports a difference, a message shows a body form this order mishandles, the loss vocabulary needs a new reason, or the converter or its version changes.
