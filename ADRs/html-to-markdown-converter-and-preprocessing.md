---
status: "accepted"
date: 2026-10-12
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# HTML-to-Markdown converter and preprocessing

*Status history: proposed and accepted 2026-10-12 (drafted after the M4e-0 bake-off; the owner accepted the recommendations below on 2026-10-12, and acceptance of this ADR is pending the by-eye review listed under Confirmation). Scope is deliberately narrow: this ADR decides the converter, the preprocessing that precedes it, and the loss policies the bake-off could inform. It does **not** decide body selection (native HTML, HTML recovered from RTF, RTF, plain text), where the original body is kept, how `cid:` images map to files in `attachments/`, or the closed vocabulary for recording formatting loss; those belong to a second body-policy ADR at M4e-2.*

## Context and Problem Statement

`message.md` is a Markdown projection of the message ([Deterministic Markdown archive](deterministic-markdown-archive.md)). In the corpus, 27 of 29 top-level `.msg` files carry their HTML only inside compressed RTF; the in-house MS-OXRTFEX de-encapsulation (M4e-1) recovers it. Turning that HTML into Markdown needs a converter, and Outlook and Word HTML is hostile input: style blocks, conditional comments, `o:p` tags, layout tables, hidden preheader text, tracking links. Which converter, what preprocessing, and which losses are accepted? The choice is expensive to reverse (a different converter, or a different version of the same one, changes the bytes of every `message.md`, so archives must be re-exported), it constrains the body pipeline (M4e-2) and the metadata that records loss (M4g), and it had real alternatives, measured in the M4e-0 bake-off.

## Decision Drivers

- No visible text may be lost, and nothing may be invented.
- Links and images must survive, with their destinations.
- Output must be deterministic, and no input may panic or fail the run.
- The result must be readable Markdown, not raw HTML or Office markup.
- A modest dependency footprint with permissive licenses.
- Loss must be explicit and known, not discovered later.
- The decision must be re-checkable cheaply when a converter changes.

## Considered Options

* **A. `htmd` in default mode, pinned, behind in-house preprocessing (chosen).**
* B. `htmd` in faithful mode.
* C. `html2markdown`.
* D. `html-to-markdown-rs`.
* E. An in-house converter (not measured).

## Decision Outcome

Chosen option: "A", because over the 27 recovered bodies and 16 synthetic and adversarial cases it was the only candidate that kept every visible word, every link, and every image with no raw HTML or Office noise left behind, and it never panicked, errored, or produced different output on a second run.

**Binding decisions:**

1. **Converter.** `htmd`, default translation mode (not faithful), pinned to exactly `=0.5.4`, the version measured. It becomes a normal (non-optional) dependency when the body pipeline (M4e-2) is built. Moving to another version is a deliberate act: re-run the bake-off harness (`--bakeoff-md`) and compare it with the figures in [`m4-results.md`](../docs/verification/m4-results.md); any change beyond timings is reviewed before the pin moves, and existing archives are re-exported.
2. **Preprocessing.** Before conversion, the HTML is preprocessed exactly as `preprocess_office_html` does at `v0.1.29`: comments, `<style>` elements, and `<script>` elements are removed with their text in one left-to-right pass (so whichever starts first in the document wins, as it would for a browser), then Word's downlevel markers (`<![if ...]>`, `<![endif]>`) and `<o:p>` tags are removed. Removing `style` and `script` text is required: without it `htmd` writes the text of body style blocks into the Markdown (21,664 extra words and 508 Office-noise markers across the corpus, against 262 and 0 after). The function moves out of the bake-off module into the body pipeline with its tests.
3. **Layout tables are flattened, and that is a recorded, accepted loss.** `htmd` produced no table rows for any of the 237 `<tr>` rows in the corpus, none of which has header cells (`<th>`); every table in the corpus is a layout table. `htmd` does convert tables it takes for data (it kept 3 of 4 rows of a small table with header cells in the synthetic set), but fidelity for real data tables is not guaranteed.
4. **Hidden text is kept** as ordinary text and not marked: 16 tags hidden by `display:none` or `mso-hide:all` occur in 7 of 27 messages and hold about 82 words, and `htmd` keeps them. Keeping can lose nothing; dropping cannot be undone.
5. **Link and image destinations are kept verbatim,** including long tracking URLs: they are 86,187 of the 264,071 output bytes (32.6%) over 361 links and 140 images. No truncation, rewriting, or removal.

### Consequences

* Good, because over the 27 recovered bodies no word is lost (re-scored with inline tags treated as the Markdown writes them), 361 of 361 links and 140 of 140 images are kept, and no Office noise or style text remains.
* Good, because the harness is a cheap, content-free fitness function for any later converter or version change.
* Good, because the dependency cost is small and permissively licensed: 22 crates beyond `htmd` itself (the `html5ever` family, `phf`, `string_cache`, `tendril`, `parking_lot`, and small helpers), of which several are build-time only; `htmd` is Apache-2.0 and the rest MIT, Apache-2.0, or both; no copyleft in the tree; the release build grew by about 5 s.
* Bad, because layout flattening means the archive is not a faithful picture of newsletter layout, and a real data table in another producer's mail may be flattened too.
* Bad, because hidden text appears in `message.md` although a reader of the original message never saw it.
* Bad, because a third of the Markdown's bytes are link destinations, mostly tracking URLs.
* Bad, because the pin may lag fixes (`htmd` 0.5.5 was available and was not measured), and the `html5ever` stack brings `markup5ever_rcdom`, whose version carries the suffix `+unofficial`; its maintenance status was not checked.
* Bad, because the word-based metric does not check order, so a reordering would pass it.

### Confirmation

* **Fitness function:** `tsp --bakeoff-md <dir of recovered HTML>` (built with `--features bakeoff` until the converter is a normal dependency). Evidence recorded so far, preprocessed variant, 27 bodies, 26,952 visible words: `htmd` 2 words missing and 262 extra, 361 of 361 links, 140 of 140 images, 31 of 32 headings, 0 table rows of 237, 1 raw HTML tag, 0 Office-noise markers, 264,071 bytes; 0 panics, errors, or nondeterministic outputs for any candidate over all 43 inputs. The 2 missing words are `month` and `¹`, a footnote anchor in its own `<span>` that `htmd` writes as `month¹` while the harness counts a word break at every tag: a metric artifact, confirmed by reading both files. Re-scored with inline tags glued (a PowerShell check, not the harness): 0 words missing in all 27 messages and 243 extra, of which 37 are numerals (list numbering), 169 are image alt text, and 37 are unexplained and in 6 messages (001, 008, 009, 023, 024, 029).
* **Rejected candidates, on the same measures:** `htmd` faithful keeps raw HTML (8,012 raw tags, 0 links, 0 headings, 73,542 extra words); `html2markdown` keeps 282 of 361 links, 73 of 140 images, and 1 of 32 headings, has 53 words missing and 514 extra, and loses a word at 300-deep nesting; `html-to-markdown-rs` keeps 46 of 140 images and 1 of 32 headings, drops hidden preheader text, has 122 words missing in 8 messages, and loses a word at 300-deep nesting.
* **Dependency and license evidence:** `cargo tree` (htmd subtree: 34 entries, 22 new beyond `htmd`) and `cargo license`, recorded in `m4-results.md`.
* **Pending before acceptance (owner):** a by-eye read of the Markdown against the HTML for 027 (three cells per row), 007 and 015 (the most extra words), and the smallest message, looking for flattened data tables, reordered text, lost links or images, and leftover CSS, tags, or entities. Only the three largest messages had been read by eye when this was drafted.
* **Planned (M4e-2):** golden tests on synthetic bodies for each variant and tricky case; a corpus run reporting per-variant conversion counts and failures; a content-preservation check that also checks order; owner review of a sample.
* **Not yet confirmed:** that order is preserved; the 37 unexplained extra words; one of 32 headings that does not appear in the Markdown (not investigated); behavior on nesting deeper than 300 and on inputs larger than the corpus; behavior on other producers (the corpus is one producer, and five of its messages share one template); `htmd` 0.5.5; the maintenance status of `markup5ever_rcdom`.

## Pros and Cons of the Options

### A. `htmd`, default mode, pinned, behind preprocessing

* Good, because it kept every word, link, and image, left no raw HTML or Office noise, flattened only layout tables, and survived every adversarial case, including 300-deep nesting.
* Good, because it is the fastest or near the fastest candidate (about 20 ms for the 27 bodies, first run each).
* Neutral, because it needs the preprocessing: raw it writes style text.
* Bad, because of the losses and risks listed under Consequences.

### B. `htmd` faithful mode

* Good, because it loses no word (0 missing).
* Bad, because it keeps HTML in place of Markdown (8,012 raw tags, no links or headings in Markdown form, 653,609 output bytes against 264,071), which defeats the purpose of `message.md`.

### C. `html2markdown`

* Good, because it also handles the adversarial cases without a panic.
* Bad, because it loses structure (282 of 361 links, 73 of 140 images, 1 of 32 headings), glues paragraphs, leaves entities in the text (514 extra words), and loses a word at deep nesting.

### D. `html-to-markdown-rs`

* Good, because it is excellent on plain articles and keeps most links (345 of 361).
* Bad, because it loses two thirds of the images and almost every heading in layout-table newsletters, drops hidden preheader text, has the most missing words (122), loses a word at deep nesting, and brings a second `html5ever` and `phf` generation to the tree.

### E. An in-house converter

* Good, because it would need no dependency and could be tuned to Outlook HTML.
* Bad, because it was not built or measured; a converter that survives the corpus and adversarial cases is a project of its own, and the measured candidate already meets the drivers.

## More Information

* Plan, stage, and open items: [`docs/plans/m4-plan.md`](../docs/plans/m4-plan.md) (M4e-0, M4e-2). Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md). Candidate research: [`docs/plans/m4a-dependency-research.md`](../docs/plans/m4a-dependency-research.md). The harness: [`src/bakeoff.rs`](../src/bakeoff.rs).
* Related: [Deterministic Markdown archive](deterministic-markdown-archive.md), [Archive file contract and schema evolution](archive-file-contract-and-schema-evolution.md) (loss is recorded, never silent).
* Follow-ups: the second body-policy ADR (body selection, the original body's placement and file names, `cid:` mapping, the formatting-loss vocabulary) at M4e-2; recording the converter's name and version and the formatting-loss reasons in `metadata.json` at M4g.
* Revisit when: the converter or its version changes, fixtures from other producers arrive, the M4e-2 content-preservation check reports a loss, or the maintenance status of the `html5ever` stack changes.
