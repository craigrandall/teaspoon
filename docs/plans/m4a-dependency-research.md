# M4a research — RTF de-encapsulation, HTML-to-Markdown, and supporting crates

Status: **research findings.** Dependencies `serde`, `serde_json`, `sha2`, `unicode-normalization`, and `proptest` (dev) are approved; the HTML-to-Markdown bake-off is approved; nothing has been added to `Cargo.toml` yet. Compiled 2026-09-30 from web searches and crate documentation. Claims marked *verify* come from search snippets or third-party summaries and must be confirmed against the crate's own source and `Cargo.toml` before adoption, in the way `compressed-rtf` was checked by reading its implementation. Crate versions and ages change quickly; treat the numbers as a snapshot.

Project constraints that filter every candidate: teaspoon is `MIT OR Apache-2.0`; output must be deterministic; the binary runs on Windows; a single maintainer means each dependency is a maintenance risk; and a crate that panics on hostile input is unacceptable (compare the `compressed-rtf` short-buffer panic found in M1).

## 1. RTF-to-HTML de-encapsulation (MS-OXRTFEX)

### Question

Is there an existing, feasible Rust crate that robustly does MS-OXRTFEX de-encapsulation (recover the original HTML, or plain text, from `PidTagRtfCompressed` after MS-OXRTFCP decompression)?

### Findings

**No. No Rust crate that does this was found.** What exists:

| Candidate | What it is | Verdict |
|---|---|---|
| `rtf-parser` (0.4.x) | General RTF lexer/parser into a document model; depends on `serde`, `wasm-bindgen`, `tsify` | Not a de-encapsulator. It reads RTF structure, not the `\htmlrtf` / `\*\htmltag` protocol. One project (`rs-chunks`) uses it to get readable text out of encapsulated RTF, which works only because it skips the `\htmltag` destinations; it discards the HTML. Heavy dependency footprint for this use. |
| `rtf-grimoire` | RTF lexer only (per `rtf-parser`'s own comparison) | Could supply tokens; would still need the whole protocol written. |
| `msg_parser` (private helper) | A hand-rolled scanner inside the crate that extracts `\htmltag` groups and skips `\htmlrtf` regions | `pub(crate)`, so not reusable. Visible in source: it works on `str::from_utf8` of the RTF, treats `\'hh` escapes as Latin-1 characters regardless of `\ansicpg`, and has the wrong LZFu dictionary tail (found in M3e). Not a model to copy. Its public `html_from_rtf()` is exactly the method M2 found not to gate on `\fromhtml1`. |
| `compressed-rtf` | MS-OXRTFCP decompression only | Already used. Not de-encapsulation. |

Implementations in other languages exist and are useful as independent references, not dependencies: `mazira/rtf-stream-parser` (Node, tokenizer plus a `DeEncapsulate` transform, explicitly targets MS-OXRTFEX; also HTML-encodes non-ASCII on output and can rewrite the charset declaration), `RTFDE` (Python; its own README says it fully unquotes text because it cannot tell RTF quoting from original quoting), and `ottertf` (Python, new; implements MS-OXRTFEX using the RTF 1.9.1 spec and the Windows code-page registry, reports damage as diagnostics instead of failing). A SwiftMail change describes the same two-step design (LZFu decompression, then honoring `\htmlrtf` fences and emitting `\*\htmltag` destinations).

### Recommendation

**Write it in-house**, as a small, pure, well-tested module, verified against the specification and against the other implementations' behavior. Reasons: nothing suitable exists; the protocol is small and specified; teaspoon already owns the surrounding code (decompression, detection, code page handling); and the failure modes (charset, quoting, malformed input) are the project's loss-accounting concern, so it should be written to the project's standards (explicit diagnostics, no panics, counts instead of silent drops).

### What the in-house module must implement (from the specification; verify each against the spec text)

1. **Recognition.** MS-OXRTFEX says a reader SHOULD check the document starts with `{\rtf1` and inspect **no more than the first 10 RTF tokens** for `\fromhtml` (encapsulated HTML) or `\fromtext` (plain text). teaspoon's current `check_compressed_rtf_bytes` looks for the `\fromhtml1` marker; confirm it matches this rule, including not matching text far into the document.
2. **Tokenizer.** Groups `{`/`}`, control words with optional numeric parameters, control symbols, and `\'hh` hex escapes; `\bin` and other binary destinations must not be mis-tokenized.
3. **`\htmlrtf` toggle.** Text and groups between `\htmlrtf` (on) and `\htmlrtf0` are RTF-only rendering and must be dropped from the HTML; content when the flag is off is HTML-bearing.
4. **`\*\htmltag` groups** carry the HTML tags; their contents are emitted verbatim. Related destinations (`\mhtmltag` for MHTML, `\*\htmltag64` and similar numbered variants) need checking against the spec.
5. **Plain text between tag groups** is HTML text content and is emitted, with `\par`, `\line`, `\tab`, and special characters converted as the spec describes.
6. **Character encoding.** `\ansicpg` sets the code page for `\'hh` bytes; `\uN` Unicode escapes are followed by a fallback character count (`\ucN`) that must be skipped. Reuse the code page decoders built for `PT_STRING8`. The HTML `<meta charset>` in the recovered HTML may disagree with the RTF code page; decide which wins and record it.
7. **Ignorable destinations** (`\*`) are skipped brace-matched.
8. **Quoting.** Decide explicitly whether to unquote RTF escapes for `{ } \` only, and never over-unquote text that was quoted in the original HTML (the `RTFDE` caveat).
9. **Attachments and `\objattph`/object placeholders** in the RTF body: record and flag; they do not map to HTML.
10. **Bounds.** Maximum group depth, maximum output size, and a hard failure mode that returns diagnostics, never panics.

### Verification plan

- Unit tests from spec examples and from hand-built streams (as the SwiftMail change did).
- Golden tests on synthetic encapsulated RTF covering each rule above.
- **Differential check** on the corpus: compare the recovered HTML text with `msg_parser`'s `html_from_rtf()` output as a *weak* oracle (counts of matches and mismatches only). Expect disagreements from its Latin-1 handling; every disagreement is triaged against the spec, as in M3e.
- Run on all 27 corpus messages that carry encapsulated HTML. Report per-file counts of tags recovered, code page used, and any diagnostics.
- Property test: for any input the function returns without panicking and within the output bound.
- Cross-check a sample by hand-comparing against one of the reference implementations' outputs (run outside the project, on non-private text).

## 2. HTML-to-Markdown converters

### Question

Which HTML-to-Markdown converters exist for Rust, what are their trade-offs, and should conversion be implemented in-house?

### Why this input is not ordinary HTML

Outlook bodies are often Word-generated HTML: `mso-` style declarations, conditional comments (`<!--[if gte mso 9]>`), Office namespaces (`<o:p>`, `<v:shape>`), layout tables, deeply nested `<span style=...>`, `<font>` tags, and `cid:` image references. A generic converter will convert what it recognizes and leave the rest as noise. Whatever converter is chosen, an in-house **preprocessing** step (strip Office namespaces and conditional comments, unwrap presentational spans, normalize layout tables, map `cid:` to attachment paths) and a **post-conversion loss report** are needed. That is where the project-specific value sits.

### Candidates

Licensing note: `html2md` is GPL-3.0-or-later. teaspoon is `MIT OR Apache-2.0`; linking a GPL crate into the binary would make the combined work GPL. Treat it as excluded.

| Crate | What it is | Pros | Cons / risks |
|---|---|---|---|
| **`htmd`** (0.5.0 at time of search) | Port of the design of turndown.js; parses with `html5ever`; Apache-2.0 *(verify)* | Claims to pass turndown.js's test cases; minimal dependencies (essentially `html5ever`); configurable options; custom element handlers (`element_handler`), which is what `cid:` mapping and loss flags need; table-to-Markdown conversion; a "faithful" mode that preserves HTML for tags Markdown cannot express, which addresses the color/highlight question; thread-safe converter; fast | Depends on `markup5ever_rcdom` (a maintenance question in the html5ever ecosystem); single-maintainer-plus-contributors project; behavior on Word HTML unknown until tested; line-wrapping and escaping choices need checking for determinism and fidelity |
| **`html2markdown`** | Two-phase AST pipeline that ports the architecture and 130 fixture tests of `hast-util-to-mdast` and `mdast-util-to-markdown`; MIT; Rust 1.80+ | Principled design (HTML tree to Markdown AST to string) with context-sensitive escaping, which matters for content that looks like Markdown syntax; separation makes custom transforms plausible | Young crate with little adoption history; extensibility for custom handlers unknown *(verify)*; may be less tolerant of odd real-world HTML than turndown-derived logic |
| **`html-to-markdown-rs`** (Kreuzberg) | Actively developed, multi-language ecosystem; MIT | Very active; rich options (heading, list, code, whitespace styles); table extraction; visitor/callback hooks; preprocessing and sanitization presets; inline image extraction; claims CommonMark compliance; benchmark-oriented | Large surface and dependency footprint; major-version churn (a 2.x API returned a `String`, 3.x returns a result struct: pinning is mandatory); conflicting statements about its parser (html5ever versus `astral-tl`) in different sources *(verify before relying on either)*; sanitization presets (aggressive removal of navigation and forms) are wrong defaults for an archive that must not drop content; designed for web scraping rather than email |
| **`h2md`** | html5ever-based; writes Markdown directly to any `Write` target; MIT *(verify)* | Streaming output; recursion depth bounded at 200 (defensive); small | Small project with little history; fewer knobs; unclear table and inline-HTML handling |
| **`html2md`** | Older converter on `html5ever`; GPL-3.0+ | Popular (many dependents) | **License excludes it.** A third-party comparison also found it produced enormous whitespace-filled output on one page |
| **`fast_html2md`** | Fork focused on speed using `lol_html`; listed as minimal maintenance | Fast; rewriting-based | "Minimal maintenance"; different engine semantics; spider-rs ecosystem; extra dependencies |
| **`html2md-rs`** | Custom (non-html5ever) parser | Small | Produced empty output for one real page in a third-party comparison; a hand-rolled parser is the wrong foundation for messy HTML |
| **`html2text`** | Renders HTML to wrapped plain text (with annotations) | Mature; good for a plain-text fallback | Not Markdown; wrapping is a rendering concern |

### Advice on writing conversion in-house

**Do not write the HTML parser or the base converter in-house.** Parsing real-world HTML the way a browser does is the hard part, `html5ever` does it, and every serious candidate uses it. A converter on top of it is a large table of element rules plus escaping, whitespace, list, and table logic; the turndown-derived and hast-derived crates encode years of test cases the project would have to rediscover.

**Do write in-house:**

1. The **Outlook/Word preprocessing** step and the `cid:` mapping.
2. The **loss report** (which constructs became Markdown, which were preserved as HTML, which were dropped).
3. A thin **`HtmlToMarkdown` trait** so the chosen crate is replaceable, plus the project's determinism rules (fixed options, pinned exact version).
4. A last-resort **fallback** for inputs the crate rejects or panics on (wrapped in `catch_unwind`, output flagged `partial`).

### Recommended process (M4e-0, a short bake-off before choosing)

1. Freeze evaluation inputs: the 27 encapsulated-HTML corpus bodies plus synthetic cases (Word cruft, tables, nested lists, entities, `cid:` images, huge inline styles, malformed tags, non-Latin text, colors/highlights).
2. Run `htmd`, `html2markdown`, and `html-to-markdown-rs` with fixed options behind the trait.
3. Score, per candidate: determinism (two runs identical), no panics on the adversarial set, preserved content (a content-preservation check: all visible text of the HTML appears in the output, order preserved), link and image fidelity, table handling, licensing, dependency count and weight (`cargo tree`), and the ability to inject custom handlers.
4. Review a sample by eye (private content, project owner only).
5. Choose, pin the exact version, and record the decision in the body-policy ADR (one of the ADRs that complete, and do not supersede, the accepted "Deterministic Markdown archive" ADR).

Provisional expectation: `htmd`, because of its custom-handler API, faithful mode, and small dependency set. This is a hypothesis to be tested, not a decision.

## 3. Supporting crates the owner approved (A6) and additions

| Need | Recommendation | Notes |
|---|---|---|
| Serialization | `serde` (with `derive`) and `serde_json` | Approved (2026-09-30). Use a struct-based schema with fixed field order; avoid `HashMap` in serialized output (order); prefer `BTreeMap` or ordered vectors for determinism. |
| Hash | **`sha2`** (SHA-256) | Approved (2026-09-30). Chosen over `blake3` because SHA-256 digests can be re-verified with tools already on Windows (`Get-FileHash`, `certutil`) and any other platform, which matters for a durable archive. `blake3` is faster but not needed. Hashing is not on a hot path. Not for security. |
| Unicode normalization | `unicode-normalization` | Approved (2026-09-30). Needed for NFC (N2) and collision keys (U1). |
| Property testing | `proptest` as a **dev-dependency** | Approved (2026-09-30). Fits the pure naming and planning functions: idempotence, uniqueness, budget, determinism. |
| YAML | Avoid | `serde_yaml` is reportedly archived; the research's suggestion is not adopted. |
| PST reading | Keep `outlook-pst` | The research suggested `libpff` FFI. Not needed here and would add native build complexity on Windows. `libpff`/`libpst` remain candidates only as differential *oracles* run outside the build. |
| Windows path specifics | None initially | The research suggested the `windows` crate to read `LongPathsEnabled` and `dunce`. Unnecessary while the budget obeys 259; revisit with `--long-paths`. |
| Parallelism | None initially | `rayon` is not needed; determinism and simplicity first. |

Every new dependency: pin an exact version, read the source for panics on hostile input, record license, run `cargo tree` for weight.

## 4. Sources

- MS-OXRTFEX (recognizing encapsulation; first 10 tokens rule), MS-OXRTFCP, MS-PST (folder objects, EntryID), MS-OXCMSG (PidTagSubject as prefix plus normalized subject) on Microsoft Learn.
- `mazira/rtf-stream-parser`, `RTFDE`, `ottertf`, the SwiftMail Outlook `.msg` change, `rs-chunks` source, `msg_parser` source, `rtf-parser` documentation.
- `htmd`, `html2markdown`, `h2md`, `html-to-markdown-rs`, `html2md`, `fast_html2md` crate pages and READMEs; a third-party 13-crate HTML-to-text comparison (2023-era, so dated).
