---
status: "accepted"
date: 2026-10-12
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Body selection, original bodies, and formatting-loss recording

*Status history: proposed 2026-10-12 (drafted for M4e-2; the owner accepted the seven recommendations it records on 2026-10-12 and then this ADR as written on 2026-10-12). It completes [HTML-to-Markdown converter and preprocessing](html-to-markdown-converter-and-preprocessing.md), which decided the converter, its pin, the preprocessing, and the policies for layout tables, hidden text, and link destinations; it amends one of them, the preprocessing list (decision 8). Not decided here: attachment extraction and the rewriting of `cid:` references to files (M4f), the closed status-reason vocabulary and the first numbered `schema_version` (M4g), and bodies read from a PST (M4i).*

## Context and Problem Statement

A message can carry its body in up to three forms: plain text (`PidTagBody`), HTML (`PidTagHtml`), and compressed RTF (`PidTagRtfCompressed`), which may encapsulate HTML. In the corpus, 27 of 29 top-level `.msg` files carry their HTML only inside the RTF (the in-house de-encapsulation, M4e-1, recovers it) and none carries native HTML. Today `message.md` holds only the plain-text body in a code fence (M4c, explicitly provisional). With the converter decided, four questions remain, and each is expensive to reverse because it fixes what an archive contains: which form becomes the body in `message.md`; which original forms the archive keeps, under what names; how a conversion that loses something says so; and how the output is verified. A fifth, smaller one came out of the by-eye review: some newsletter templates put about 600 invisible characters of padding into every message.

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
3. **`cid:` references stay as written** in `message.md`. The number of `cid:` references and of distinct ones is recorded in `metadata.json`. Rewriting them to relative `attachments/` paths belongs to M4f, when the attachment files exist.
4. **Loss is recorded in a `body` object in `metadata.json`** (draft schema `0.1-draft`, frozen at M4g under the evolution policy): the source; for HTML sources, the converter's name and version (`htmd`, `0.5.4`); the preprocessing steps applied, from a closed list (`comments`, `style`, `script`, `title`, `downlevel_markers`, `office_paragraph_tags`, `invisible_padding`); the kept original files; and `loss`, a list of `{reason, count}` from a closed vocabulary that starts as: `tables_flattened` (table rows in the HTML that have no Markdown table row), `rtf_not_converted` (an RTF existed that could not be used), `undecodable_bytes_replaced` (bytes the de-encapsulation could not decode, replaced by U+FFFD), and `invisible_padding_removed` (the number of runs removed). A new reason is a schema change. How reasons roll up into the message's `partial` status and its reasons is decided with the status vocabulary at M4g; until then, the M4c status reason for unconverted formatted bodies is no longer recorded for a message whose body was converted.
5. **Invisible padding is collapsed in `message.md` only.** After conversion, a run of at least 8 characters from {U+034F, U+00AD, U+200B, U+200C, U+200D, U+2060, U+FEFF}, with any spaces and no-break spaces between and around them, is removed; a line left empty is removed, and consecutive blank lines left behind collapse to one. Shorter runs, and the direction marks U+200E and U+200F, stay. Each run removed is counted as `invisible_padding_removed`. The threshold is conservative: the runs seen are 200 characters long, and emoji joiner sequences are never 8 invisible characters in a row.
6. **A content-preservation check** is part of the product: a pure function and a counts-only corpus mode (`tsp --verify-bodies <input>`) that compare, in order, the visible words of the HTML (comments, `head`, `style`, and `script` excluded; inline tags with no Markdown delimiter glued) with the words of the Markdown (image alt text, link destinations, ordered-list marker numerals, and removed padding excluded). It prints counts only: words in the HTML, words in the Markdown, words matched in order, and the number of differences. The documented exclusions are the only ones. The gate is no difference over the corpus. It is implemented in-house (no new dependency) and unit-tested.
7. **Work items, not decisions:** `htmd` becomes a normal dependency pinned `=0.5.4`; `preprocess_office_html` moves out of the bake-off module with its tests; golden tests cover each source and the tricky cases (empty body, plain text only, RTF without encapsulated HTML, an unterminated style element, padding runs, `cid:` images).

8. **The preprocessing also removes `<title>` elements** (with their text, in the same single pass as comments, `style`, and `script`), which amends the converter ADR's preprocessing list; that ADR records the amendment. Evidence: `htmd` writes a non-empty `<title>` as the first line of the Markdown. The owner checked six messages: in 001, 008, 009, 023, and 024 the first line of the Markdown equals the `<title>` text (11, 3, 4, 8, and 3 words), accounting for 28 of the 37 extra words the harness could not otherwise explain, and 029 has no title. The title is not body text and repeats the subject, which `message.md` already shows as its heading. The harness uses the same preprocessing function, so its fitness function stays aligned; the expected effect of a re-run is about 28 fewer extra words, nothing else changed, and it is to be confirmed. Only `<title>` is removed, not the whole `<head>`, because an unterminated `<head>` would drop the body, and an unterminated `<title>` is by the HTML rules title text to the end of the document.

### Consequences

* Good, because every form the message had is in the archive, so a better converter can re-derive `message.md` from `body.html` and `body.rtf`, and nothing is lost to a conversion bug.
* Good, because loss has names and counts in metadata, and the content check is reproducible and content-free.
* Good, because `message.md` loses the document title and the invisible padding (about 1.5 KB per message in the shared newsletter template) while the original keeps it.
* Bad, because the archive grows: the HTML (40 to 65 KB for the larger corpus messages) and the RTF, which is larger than the HTML it carries, are stored beside the Markdown.
* Bad, because `body.rtf` and a non-UTF-8 `body.html` are not text the tool can read back as UTF-8, and they are exempt from the generated-file rules.
* Bad, because `cid:` images do not display from `message.md` until M4f.
* Bad, because the loss vocabulary and the `body` object are drafts until M4g, and the padding threshold and the exclusion list in the check are judgments that real mail may challenge.

### Confirmation

* **Evidence so far:** the by-eye review of four pairs (007, 015, 022, 027), recorded in the converter ADR and `m4-results.md`: the word sequence of the Markdown equals the HTML's, in order, with the exclusions above, in all four; the padding runs (200 U+034F, 200 U+00AD, about 200 no-break spaces) in 007 and 027; the `cid:` references in 015 and 007; the one U+FFFD (015) that is already in the recovered HTML.
* **Planned (M4e-2):** golden tests; `--verify-bodies` over the 29-message corpus with no difference, and its exclusions listed; the exported tree reconciled with the counts (every message has a `body` object, every kept file's hash matches its metadata); two exports byte-identical; owner review of a sample.
* **Title evidence (done 2026-10-12):** in 001, 008, 009, 023, and 024 the first line of the Markdown equals the `<title>` text; 029 has no title; 007, 015, 022, and 027 have none or an empty one. The Cyrillic words in 023's extra-word listing are image alt text that is entity-encoded in the HTML (`alt` attributes with numeric entities were found in 023 and 008), so the listing script missed them; one extra word in 029 is unexplained.
* **Not yet confirmed:** the native-HTML path on real data (the corpus has none; synthetic tests cover it); an RTF without encapsulated HTML on real data (one `\fromtext` file, not handled); bodies from other producers; the padding threshold on mail other than this template; whether `tables_flattened` should count tables or rows.

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
* Follow-ups: rewriting `cid:` references and extracting attachments (M4f); the status-reason vocabulary and the first numbered schema (M4g); PST bodies (M4i).
* Revisit when: the content check reports a difference, a message shows a body form this order mishandles, the loss vocabulary needs a new reason, or the converter or its version changes.
