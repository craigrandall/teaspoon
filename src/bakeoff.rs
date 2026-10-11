//! HTML-to-Markdown bake-off (M4e-0): runs each candidate converter over the HTML recovered by
//! the in-house de-encapsulation (the `NNN.custom.html` files `--dump-deencap` writes) and over a
//! built-in set of synthetic and adversarial cases, and prints content-free scores.
//!
//! Candidates (compiled only with `--features bakeoff`, so the default build and its
//! dependencies are unchanged): `htmd` (pure and faithful mode), `html2markdown`, and
//! `html-to-markdown-rs`. Each is run in a worker thread with a large stack, twice per input
//! (determinism), on the raw HTML and on a lightly preprocessed copy (Word comments and `o:p`
//! tags removed), and judged on:
//!
//! - **robustness**: panics and errors (a panic is caught as a thread failure, never fatal);
//! - **determinism**: the two runs must produce identical output;
//! - **content preservation**: how many of the HTML's visible words are missing from the
//!   Markdown, counting in order within a bounded window (a heuristic: link destinations add
//!   words to the Markdown but never hide missing ones);
//! - **structure**: links, images, table rows, headings, and list items in the HTML against what
//!   appears in the Markdown, plus raw HTML tags left behind, Office noise left behind, and
//!   U+FFFD characters;
//! - **cost**: total time and output size.
//!
//! What is printed is counts only. With `--bakeoff-dump <dir>` the Markdown each candidate wrote
//! is saved locally for review (the only place content goes). Everything that scores is pure and
//! unit-tested with stand-in converters, so the harness can be trusted before any candidate is.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};

/// Stack for each conversion thread (Windows' default main-thread stack is only 1 MiB, and
/// converters recurse on nesting depth).
const STACK_BYTES: usize = 32 * 1024 * 1024;
/// How far ahead in the Markdown's words a word of the HTML is looked for.
const WORD_WINDOW: usize = 200;

/// One converter under test. Implementations are stateless, so each call builds what it needs.
pub(crate) trait MarkdownConverter: Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn convert(&self, html: &str) -> Result<String, String>;
}

// =============================================================================
// Candidate adapters (feature `bakeoff`)
// =============================================================================

#[cfg(feature = "bakeoff")]
mod adapters {
    use std::sync::Arc;

    use super::MarkdownConverter;

    pub(super) struct Htmd;
    pub(super) struct HtmdFaithful;
    pub(super) struct Html2Markdown;
    pub(super) struct HtmlToMarkdownRs;

    impl MarkdownConverter for Htmd {
        fn name(&self) -> &'static str {
            "htmd"
        }
        fn version(&self) -> &'static str {
            "0.5.4"
        }
        fn convert(&self, html: &str) -> Result<String, String> {
            htmd::convert(html).map_err(|e| e.to_string())
        }
    }

    impl MarkdownConverter for HtmdFaithful {
        fn name(&self) -> &'static str {
            "htmd_faithful"
        }
        fn version(&self) -> &'static str {
            "0.5.4"
        }
        fn convert(&self, html: &str) -> Result<String, String> {
            let converter = htmd::HtmlToMarkdown::builder()
                .options(htmd::options::Options {
                    translation_mode: htmd::options::TranslationMode::Faithful,
                    ..Default::default()
                })
                .build();
            converter.convert(html).map_err(|e| e.to_string())
        }
    }

    impl MarkdownConverter for Html2Markdown {
        fn name(&self) -> &'static str {
            "html2markdown"
        }
        fn version(&self) -> &'static str {
            "0.2.0"
        }
        fn convert(&self, html: &str) -> Result<String, String> {
            Ok(html2markdown::convert(html))
        }
    }

    impl MarkdownConverter for HtmlToMarkdownRs {
        fn name(&self) -> &'static str {
            "html_to_markdown_rs"
        }
        fn version(&self) -> &'static str {
            "3.11.0"
        }
        fn convert(&self, html: &str) -> Result<String, String> {
            html_to_markdown_rs::convert(html, None)
                .map(|result| result.content.unwrap_or_default())
                .map_err(|e| e.to_string())
        }
    }

    pub(super) fn all() -> Vec<Arc<dyn MarkdownConverter>> {
        vec![
            Arc::new(Htmd),
            Arc::new(HtmdFaithful),
            Arc::new(Html2Markdown),
            Arc::new(HtmlToMarkdownRs),
        ]
    }
}

#[cfg(feature = "bakeoff")]
fn candidates() -> Vec<Arc<dyn MarkdownConverter>> {
    adapters::all()
}

#[cfg(not(feature = "bakeoff"))]
fn candidates() -> Vec<Arc<dyn MarkdownConverter>> {
    Vec::new()
}

// =============================================================================
// Reading HTML: tags, visible text, words
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
struct TagToken {
    /// Lowercase tag name, possibly empty (`<!DOCTYPE ...>`).
    name: String,
    close: bool,
    /// An opening tag that carries `href=`.
    has_href: bool,
}

/// Walks HTML, calling `on_tag` for each tag and `on_text` for each run of text. Comments are
/// skipped, and so is everything inside `style`, `script`, `head`, and `title` elements. A `<`
/// that does not start a tag is text. A heuristic reader for scoring, not an HTML parser.
fn walk(html: &str, on_tag: &mut dyn FnMut(&TagToken), on_text: &mut dyn FnMut(&str)) {
    let lower = html.to_ascii_lowercase();
    let len = html.len();
    let mut i = 0usize;
    while i < len {
        let Some(relative) = html[i..].find('<') else {
            on_text(&html[i..]);
            break;
        };
        let lt = i + relative;
        if lt > i {
            on_text(&html[i..lt]);
        }
        if html[lt..].starts_with("<!--") {
            i = html[lt + 4..]
                .find("-->")
                .map_or(len, |position| lt + 4 + position + 3);
            continue;
        }
        let after = &html[lt + 1..];
        let starts_tag = matches!(
            after.chars().next(),
            Some(c) if c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?'
        );
        if !starts_tag {
            on_text("<");
            i = lt + 1;
            continue;
        }
        let end = html[lt..]
            .find('>')
            .map_or(len, |position| lt + position + 1);
        let close = after.starts_with('/');
        let name_source = if close { &after[1..] } else { after };
        let name: String = name_source
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let has_href = !close && lower[lt..end].contains("href=");
        on_tag(&TagToken {
            name: name.clone(),
            close,
            has_href,
        });
        on_text(" ");
        i = end;
        if !close && matches!(name.as_str(), "style" | "script" | "head" | "title") {
            let closing = format!("</{name}");
            i = lower[end..].find(&closing).map_or(len, |position| {
                let at = end + position;
                lower[at..].find('>').map_or(len, |q| at + q + 1)
            });
        }
    }
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "nbsp" => '\u{00A0}',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "eacute" => '\u{e9}',
        "egrave" => '\u{e8}',
        "ecirc" => '\u{ea}',
        "agrave" => '\u{e0}',
        "aacute" => '\u{e1}',
        "ccedil" => '\u{e7}',
        "iacute" => '\u{ed}',
        "oacute" => '\u{f3}',
        "uacute" => '\u{fa}',
        "ntilde" => '\u{f1}',
        "uuml" => '\u{fc}',
        "ouml" => '\u{f6}',
        "auml" => '\u{e4}',
        "szlig" => '\u{df}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "hellip" => '\u{2026}',
        "bull" => '\u{2022}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "euro" => '\u{20AC}',
        _ => return None,
    })
}

/// Decodes the common named entities and all numeric ones; anything else is left as written.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let candidate = &rest[amp + 1..];
        let decoded = candidate
            .find(';')
            .filter(|end| *end <= 12)
            .and_then(|end| {
                let body = &candidate[..end];
                let ch = if let Some(hex) =
                    body.strip_prefix("#x").or_else(|| body.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(decimal) = body.strip_prefix('#') {
                    decimal.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    named_entity(body)
                };
                ch.map(|c| (c, end))
            });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &candidate[end + 1..];
            }
            None => {
                out.push('&');
                rest = candidate;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Maximal runs of alphanumeric characters.
fn alnum_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            current.push(c);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// What the HTML contains, for comparison with what the Markdown kept.
#[derive(Default, Debug)]
struct HtmlFacts {
    words: Vec<String>,
    links: u64,
    images: u64,
    table_rows: u64,
    headings: u64,
    list_items: u64,
}

fn html_facts(html: &str) -> HtmlFacts {
    let mut facts = HtmlFacts::default();
    let mut text = String::new();
    walk(
        html,
        &mut |tag| {
            if tag.close {
                return;
            }
            match tag.name.as_str() {
                "a" if tag.has_href => facts.links += 1,
                "img" => facts.images += 1,
                "tr" => facts.table_rows += 1,
                "li" => facts.list_items += 1,
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => facts.headings += 1,
                _ => {}
            }
        },
        &mut |piece| text.push_str(piece),
    );
    facts.words = alnum_words(&decode_entities(&text));
    facts
}

/// How many of `html_words` are not found, in order, in `markdown_words`: each word is looked
/// for within the next [`WORD_WINDOW`] words after the last match; a word not found is counted
/// and the position does not move.
fn ordered_missing(html_words: &[String], markdown_words: &[String]) -> u64 {
    let mut position = 0usize;
    let mut missing = 0u64;
    for word in html_words {
        let end = (position + WORD_WINDOW).min(markdown_words.len());
        match markdown_words[position..end].iter().position(|m| m == word) {
            Some(offset) => position += offset + 1,
            None => missing += 1,
        }
    }
    missing
}

// =============================================================================
// Reading Markdown: structure and noise
// =============================================================================

#[derive(Default, Debug, PartialEq, Eq)]
struct MarkdownCounts {
    links: u64,
    images: u64,
    table_rows: u64,
    headings: u64,
    list_items: u64,
    raw_html_tags: u64,
    noise_markers: u64,
    replacement_chars: u64,
}

fn is_list_item(trimmed: &str) -> bool {
    if trimmed.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) {
        return false; // a horizontal rule
    }
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        return true;
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") "))
}

fn markdown_counts(markdown: &str) -> MarkdownCounts {
    let images = markdown.matches("![").count() as u64;
    let links = (markdown.matches("](").count() as u64).saturating_sub(images);
    let mut counts = MarkdownCounts {
        links,
        images,
        ..MarkdownCounts::default()
    };
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('|') && !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
        {
            counts.table_rows += 1;
        }
        let atx = line.len() - trimmed.len() < 4
            && trimmed.starts_with('#')
            && trimmed.trim_start_matches('#').starts_with(' ');
        let underline = {
            let t = line.trim();
            t.len() >= 2 && t.chars().all(|c| c == '=')
        };
        if atx || underline {
            counts.headings += 1;
        }
        if is_list_item(trimmed) {
            counts.list_items += 1;
        }
    }
    let bytes = markdown.as_bytes();
    counts.raw_html_tags = bytes
        .windows(2)
        .filter(|pair| pair[0] == b'<' && (pair[1].is_ascii_alphabetic() || pair[1] == b'/'))
        .count() as u64;
    counts.noise_markers = ["mso-", "<o:p", "[if ", "<![endif", "xmlns:"]
        .iter()
        .map(|marker| markdown.matches(marker).count() as u64)
        .sum();
    counts.replacement_chars = markdown.matches('\u{FFFD}').count() as u64;
    counts
}

// =============================================================================
// Preprocessing (light, for the "pre" variant)
// =============================================================================

fn strip_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Removes `<![if ...]>` and `<![endif]>` markers (Word's downlevel-revealed sections).
fn strip_marked_sections(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<![") {
        out.push_str(&rest[..start]);
        match rest[start..].find("]>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Removes `<o:p>` and `</o:p>` tags, keeping what is inside.
fn remove_office_paragraph_tags(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        let Some(relative) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + relative;
        out.push_str(&html[i..lt]);
        let tail = &lower[lt + 1..];
        let after_slash = tail.strip_prefix('/').unwrap_or(tail);
        let is_office_p = after_slash.starts_with("o:p")
            && after_slash[3..]
                .chars()
                .next()
                .is_some_and(|c| c == '>' || c == '/' || c.is_whitespace());
        if is_office_p {
            i = html[lt..].find('>').map_or(html.len(), |end| lt + end + 1);
        } else {
            out.push('<');
            i = lt + 1;
        }
    }
    out
}

/// The light preprocessing the "pre" variant applies before conversion.
pub(crate) fn preprocess_office_html(html: &str) -> String {
    remove_office_paragraph_tags(&strip_marked_sections(&strip_comments(html)))
}

// =============================================================================
// Running and scoring
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Variant {
    Raw,
    Preprocessed,
}

impl Variant {
    fn label(self) -> &'static str {
        match self {
            Variant::Raw => "raw",
            Variant::Preprocessed => "pre",
        }
    }
}

#[derive(Default, Clone, Debug)]
struct Measure {
    inputs: u64,
    panics: u64,
    errors: u64,
    nondeterministic: u64,
    output_bytes: u64,
    millis: u64,
    words_html: u64,
    words_missing: u64,
    links_html: u64,
    links_md: u64,
    images_html: u64,
    images_md: u64,
    table_rows_html: u64,
    table_rows_md: u64,
    headings_html: u64,
    headings_md: u64,
    list_items_html: u64,
    list_items_md: u64,
    raw_html_tags_md: u64,
    noise_markers_md: u64,
    replacement_chars_md: u64,
}

struct Outcome {
    markdown: Option<String>,
    panicked: bool,
    errored: bool,
    millis: u64,
}

/// Runs one conversion on its own thread with a large stack. A panic is the thread's failure,
/// not the process's.
fn run_converter(converter: &Arc<dyn MarkdownConverter>, html: &str) -> Outcome {
    let converter = Arc::clone(converter);
    let owned = html.to_string();
    let started = Instant::now();
    let spawned = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || converter.convert(&owned));
    let Ok(handle) = spawned else {
        return Outcome {
            markdown: None,
            panicked: false,
            errored: true,
            millis: 0,
        };
    };
    let joined = handle.join();
    let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match joined {
        Ok(Ok(markdown)) => Outcome {
            markdown: Some(markdown),
            panicked: false,
            errored: false,
            millis,
        },
        Ok(Err(_)) => Outcome {
            markdown: None,
            panicked: false,
            errored: true,
            millis,
        },
        Err(_) => Outcome {
            markdown: None,
            panicked: true,
            errored: false,
            millis,
        },
    }
}

/// Converts `input` twice with `converter`, folds the result into `measure`, and returns the
/// first run's Markdown. `facts` describe the original HTML, whatever `input` was.
fn evaluate(
    measure: &mut Measure,
    converter: &Arc<dyn MarkdownConverter>,
    facts: &HtmlFacts,
    input: &str,
) -> Option<String> {
    measure.inputs += 1;
    let first = run_converter(converter, input);
    let second = run_converter(converter, input);
    measure.millis += first.millis;
    measure.panics += u64::from(first.panicked);
    measure.errors += u64::from(first.errored);
    if first.markdown != second.markdown {
        measure.nondeterministic += 1;
    }
    let markdown = first.markdown?;

    measure.output_bytes += markdown.len() as u64;
    measure.words_html += facts.words.len() as u64;
    measure.words_missing += ordered_missing(&facts.words, &alnum_words(&markdown));
    let counts = markdown_counts(&markdown);
    measure.links_html += facts.links;
    measure.links_md += counts.links;
    measure.images_html += facts.images;
    measure.images_md += counts.images;
    measure.table_rows_html += facts.table_rows;
    measure.table_rows_md += counts.table_rows;
    measure.headings_html += facts.headings;
    measure.headings_md += counts.headings;
    measure.list_items_html += facts.list_items;
    measure.list_items_md += counts.list_items;
    measure.raw_html_tags_md += counts.raw_html_tags;
    measure.noise_markers_md += counts.noise_markers;
    measure.replacement_chars_md += counts.replacement_chars;
    Some(markdown)
}

// =============================================================================
// Inputs
// =============================================================================

/// The built-in synthetic and adversarial cases (kept modest in depth so a converter that
/// overflows its stack on pathological nesting cannot take the whole run down).
fn synthetic_cases() -> Vec<String> {
    let deep = format!("{}deep{}", "<div>".repeat(300), "</div>".repeat(300));
    let many: String = (1..=2000).map(|n| format!("<p>item {n}</p>")).collect();
    vec![
        String::new(),
        "Just text, no tags.".to_string(),
        concat!(
            "<html xmlns:o=\"urn:schemas-microsoft-com:office:office\"><head>",
            "<!--[if gte mso 9]><xml><o:OfficeDocumentSettings><o:AllowPNG/>",
            "</o:OfficeDocumentSettings></xml><![endif]-->",
            "<style>p.MsoNormal{margin:0in;mso-line-height-rule:exactly}</style></head>",
            "<body><p class=MsoNormal><span style='font-size:11.0pt;mso-bidi-font-size:10.0pt'>",
            "Hello <o:p></o:p>Word cruft</span><o:p>&nbsp;</o:p></p>",
            "<![if !supportLists]><span>1.</span><![endif]></body></html>"
        )
        .to_string(),
        "<ul><li>one<ul><li>two<ol><li>three</li><li>four</li></ol></li></ul></li><li>five</li></ul>"
            .to_string(),
        "<table border=1><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr><tr><td colspan=2>wide</td></tr></table>"
            .to_string(),
        "<p>Caf&eacute; &amp; cr&egrave;me &lt;tag&gt; &#8364;5 &#x2603; &nbsp;done</p>"
            .to_string(),
        "<p>See <a href=\"https://example.com/a?b=1&amp;c=2\">the page</a> and <img src=\"cid:image001.png@01D00000.00000000\" alt=\"logo\"> then <a href=\"mailto:a@example.com\">mail</a>.</p>"
            .to_string(),
        "<p>unclosed <b>bold <i>italic</p><div>stray </span> text<table><tr><td>cell</div>"
            .to_string(),
        "<p dir=\"auto\">\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{30C6}\u{30AD}\u{30B9}\u{30C8} \u{41F}\u{440}\u{438}\u{432}\u{435}\u{442} \u{43C}\u{438}\u{440} \u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD} \u{645}\u{631}\u{62D}\u{628}\u{627}</p>"
            .to_string(),
        "<p><span style=\"color:#ff0000;background-color:#ffff00\">highlighted red</span> <font color=\"blue\">blue</font> <mark>marked</mark></p>"
            .to_string(),
        format!("<p style=\"{}\">styled</p>", "a:b;".repeat(5000)),
        deep,
        many,
        "<p>before</p><script>var x = \"<b>not text</b>\";</script><style>p{color:red}</style><p>after</p>"
            .to_string(),
        "<h1>Title</h1><h2>Sub</h2><p><strong>bold</strong> <em>italic</em> <code>code</code></p><blockquote>quote</blockquote><pre>pre\n  formatted</pre><hr>"
            .to_string(),
        "<p>1. not a list * not bullet # not heading _under_ [brackets](paren) `tick` | pipe</p>"
            .to_string(),
    ]
}

/// The `NNN.custom.html` files of a `--dump-deencap` directory, with their numbers.
fn read_corpus(directory: &Path) -> Result<(Vec<(usize, String)>, u64)> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .context("failed to read the bake-off input directory")?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".custom.html"))
        .collect();
    names.sort();
    if names.is_empty() {
        anyhow::bail!("the bake-off input directory contains no .custom.html files");
    }
    let mut inputs = Vec::new();
    let mut read_errors = 0u64;
    for (position, name) in names.iter().enumerate() {
        let index = name
            .split('.')
            .next()
            .and_then(|digits| digits.parse::<usize>().ok())
            .unwrap_or(position + 1);
        match fs::read_to_string(directory.join(name)) {
            Ok(html) => inputs.push((index, html)),
            Err(_) => read_errors += 1,
        }
    }
    Ok((inputs, read_errors))
}

/// A new directory is created; an existing one must be empty. Errors never include the path.
fn prepare_output_dir(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        if !path.is_dir() {
            anyhow::bail!("the --bakeoff-dump path exists and is not a directory");
        }
        let mut entries =
            fs::read_dir(path).context("failed to read the --bakeoff-dump directory")?;
        if entries.next().is_some() {
            anyhow::bail!(
                "the --bakeoff-dump directory is not empty; name a new or empty directory"
            );
        }
    } else {
        fs::create_dir_all(path).context("failed to create the --bakeoff-dump directory")?;
    }
    Ok(path.to_path_buf())
}

struct Dump {
    path: PathBuf,
    files_written: u64,
}

impl Dump {
    fn write(&mut self, name: &str, text: &str) -> Result<()> {
        fs::write(self.path.join(name), text.as_bytes())
            .context("failed to write a --bakeoff-dump file")?;
        self.files_written += 1;
        Ok(())
    }
}

// =============================================================================
// The run and the report
// =============================================================================

type Totals = BTreeMap<(&'static str, &'static str, Variant), Measure>;

fn run_set(
    set: &'static str,
    inputs: &[(String, String)],
    converters: &[Arc<dyn MarkdownConverter>],
    totals: &mut Totals,
    dump: &mut Option<Dump>,
) -> Result<()> {
    for (label, html) in inputs {
        let facts = html_facts(html);
        let preprocessed = preprocess_office_html(html);
        for converter in converters {
            for variant in [Variant::Raw, Variant::Preprocessed] {
                let input: &str = match variant {
                    Variant::Raw => html.as_str(),
                    Variant::Preprocessed => preprocessed.as_str(),
                };
                let measure = totals.entry((set, converter.name(), variant)).or_default();
                let markdown = evaluate(measure, converter, &facts, input);
                if let (Some(dump), Some(markdown)) = (dump.as_mut(), markdown) {
                    let name = format!("{label}.{}.{}.md", converter.name(), variant.label());
                    dump.write(&name, &markdown)?;
                }
            }
        }
    }
    Ok(())
}

fn run_everything(
    corpus: &[(String, String)],
    synthetic: &[(String, String)],
    converters: &[Arc<dyn MarkdownConverter>],
    totals: &mut Totals,
    dump: &mut Option<Dump>,
) -> Result<()> {
    if let Some(dump) = dump.as_mut() {
        for (label, html) in synthetic {
            dump.write(&format!("{label}.html"), html)?;
        }
    }
    run_set("corpus", corpus, converters, totals, dump)?;
    run_set("synthetic", synthetic, converters, totals, dump)
}

fn print_measure(prefix: &str, m: &Measure) {
    println!("{prefix}_inputs={}", m.inputs);
    println!("{prefix}_panics={}", m.panics);
    println!("{prefix}_errors={}", m.errors);
    println!("{prefix}_nondeterministic={}", m.nondeterministic);
    println!("{prefix}_output_bytes={}", m.output_bytes);
    println!("{prefix}_millis={}", m.millis);
    println!("{prefix}_words_html={}", m.words_html);
    println!("{prefix}_words_missing={}", m.words_missing);
    println!("{prefix}_links_html={}", m.links_html);
    println!("{prefix}_links_md={}", m.links_md);
    println!("{prefix}_images_html={}", m.images_html);
    println!("{prefix}_images_md={}", m.images_md);
    println!("{prefix}_table_rows_html={}", m.table_rows_html);
    println!("{prefix}_table_rows_md={}", m.table_rows_md);
    println!("{prefix}_headings_html={}", m.headings_html);
    println!("{prefix}_headings_md={}", m.headings_md);
    println!("{prefix}_list_items_html={}", m.list_items_html);
    println!("{prefix}_list_items_md={}", m.list_items_md);
    println!("{prefix}_raw_html_tags_md={}", m.raw_html_tags_md);
    println!("{prefix}_noise_markers_md={}", m.noise_markers_md);
    println!("{prefix}_replacement_chars_md={}", m.replacement_chars_md);
}

/// Runs the bake-off over the `NNN.custom.html` files in `input_dir` and the synthetic cases.
pub(crate) fn run_bakeoff(input_dir: &Path, dump_dir: Option<&Path>) -> Result<()> {
    let converters = candidates();
    if converters.is_empty() {
        anyhow::bail!(
            "the bake-off converters are not compiled in; rebuild with `--features bakeoff`"
        );
    }
    let mut dump = dump_dir
        .map(|path| {
            prepare_output_dir(path).map(|path| Dump {
                path,
                files_written: 0,
            })
        })
        .transpose()?;
    let (corpus, read_errors) = read_corpus(input_dir)?;
    let corpus: Vec<(String, String)> = corpus
        .into_iter()
        .map(|(index, html)| (format!("{index:03}"), html))
        .collect();
    let synthetic: Vec<(String, String)> = synthetic_cases()
        .into_iter()
        .enumerate()
        .map(|(n, html)| (format!("s{:02}", n + 1), html))
        .collect();

    // A converter that panics would otherwise print its panic message (which can quote input).
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut totals = Totals::new();
    let result = run_everything(&corpus, &synthetic, &converters, &mut totals, &mut dump);
    std::panic::set_hook(previous_hook);
    result?;

    println!("inventory=privacy_safe");
    println!("input_kind=html_bakeoff");
    println!("corpus_inputs={}", corpus.len());
    println!("corpus_read_errors={read_errors}");
    println!("synthetic_inputs={}", synthetic.len());
    for converter in &converters {
        println!(
            "candidate_{}_version={}",
            converter.name(),
            converter.version()
        );
    }
    for ((set, name, variant), measure) in &totals {
        print_measure(&format!("{set}_{name}_{}", variant.label()), measure);
    }
    if let Some(dump) = &dump {
        println!("bakeoff_dump_files_written={}", dump.files_written);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// Returns the HTML's visible words joined: a perfect converter for scoring purposes.
    struct Echo;
    impl MarkdownConverter for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn version(&self) -> &'static str {
            "0"
        }
        fn convert(&self, html: &str) -> Result<String, String> {
            Ok(html_facts(html).words.join(" "))
        }
    }

    struct Panics;
    impl MarkdownConverter for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn version(&self) -> &'static str {
            "0"
        }
        fn convert(&self, _html: &str) -> Result<String, String> {
            panic!("boom");
        }
    }

    struct Fails;
    impl MarkdownConverter for Fails {
        fn name(&self) -> &'static str {
            "fails"
        }
        fn version(&self) -> &'static str {
            "0"
        }
        fn convert(&self, _html: &str) -> Result<String, String> {
            Err("no".to_string())
        }
    }

    struct Flaky(AtomicU64);
    impl MarkdownConverter for Flaky {
        fn name(&self) -> &'static str {
            "flaky"
        }
        fn version(&self) -> &'static str {
            "0"
        }
        fn convert(&self, _html: &str) -> Result<String, String> {
            Ok(format!("run {}", self.0.fetch_add(1, Ordering::SeqCst)))
        }
    }

    fn arc(converter: impl MarkdownConverter + 'static) -> Arc<dyn MarkdownConverter> {
        Arc::new(converter)
    }

    #[test]
    fn entities_are_decoded_and_unknown_ones_are_left_alone() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(
            decode_entities("caf&eacute; &#233; &#xE9;"),
            "caf\u{e9} \u{e9} \u{e9}"
        );
        assert_eq!(decode_entities("&bogus; & plain"), "&bogus; & plain");
        assert_eq!(decode_entities("no entity"), "no entity");
        assert_eq!(decode_entities("trailing &"), "trailing &");
    }

    #[test]
    fn visible_words_skip_comments_head_style_and_script() {
        let html = "<html><head><title>T</title><style>p{color:red}</style></head><body><!-- hidden words --><p>Hello <b>big</b> world</p><script>var hidden = 1;</script>after</body></html>";
        assert_eq!(
            html_facts(html).words,
            words(&["Hello", "big", "world", "after"])
        );
    }

    #[test]
    fn a_less_than_sign_that_starts_no_tag_is_text() {
        assert_eq!(
            html_facts("<p>a < b and c</p>").words,
            words(&["a", "b", "and", "c"])
        );
    }

    #[test]
    fn entities_inside_text_become_part_of_words() {
        assert_eq!(
            html_facts("<p>Caf&eacute;&nbsp;au lait</p>").words,
            words(&["Caf\u{e9}", "au", "lait"])
        );
    }

    #[test]
    fn tags_are_counted_by_kind() {
        let html = "<h1>T</h1><a href=\"x\">l</a><a name=\"y\">n</a><img src=a><table><tr><td>1</td></tr><tr><td>2</td></tr></table><ul><li>a</li><li>b</li><li>c</li></ul>";
        let facts = html_facts(html);
        assert_eq!(
            (
                facts.links,
                facts.images,
                facts.table_rows,
                facts.headings,
                facts.list_items
            ),
            (1, 1, 2, 1, 3)
        );
    }

    #[test]
    fn missing_words_are_counted_in_order_within_a_window() {
        let html = words(&["a", "b", "c", "d"]);
        assert_eq!(ordered_missing(&html, &words(&["a", "b", "c", "d"])), 0);
        assert_eq!(ordered_missing(&html, &words(&["a", "c", "d"])), 1);
        assert_eq!(ordered_missing(&html, &words(&[])), 4);
        // Out of order: "b" is found after "c", so "c" is then missing.
        assert_eq!(
            ordered_missing(&words(&["a", "c", "b"]), &words(&["a", "b", "c"])),
            1
        );
        // Extra words in the Markdown (link destinations) never hide a missing one.
        assert_eq!(
            ordered_missing(&html, &words(&["a", "x", "b", "y", "c", "d"])),
            0
        );
    }

    #[test]
    fn markdown_structure_is_counted() {
        let md = "# Title\n\nSub\n===\n\n- one\n* two\n1. three\n- - -\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n[l](http://x) ![i](y.png) <span>x</span>\n";
        let c = markdown_counts(md);
        assert_eq!(c.headings, 2);
        assert_eq!(c.list_items, 3);
        assert_eq!(c.table_rows, 2);
        assert_eq!((c.links, c.images), (1, 1));
        assert_eq!(c.raw_html_tags, 2);
    }

    #[test]
    fn office_noise_and_replacement_characters_are_counted() {
        let c = markdown_counts("a mso-bidi <o:p> [if gte mso 9] \u{FFFD}\u{FFFD}");
        assert_eq!(c.noise_markers, 3);
        assert_eq!(c.replacement_chars, 2);
    }

    #[test]
    fn preprocessing_removes_word_comments_markers_and_office_paragraph_tags() {
        let html = "a<!--[if gte mso 9]><xml>x</xml><![endif]-->b<![if !supportLists]>c<![endif]>d<o:p>e</o:p>f<O:P/>g<p>h</p>";
        assert_eq!(preprocess_office_html(html), "abcdefg<p>h</p>");
        // Something that only looks like it: left alone.
        assert_eq!(
            preprocess_office_html("<o:pen>x</o:pen>"),
            "<o:pen>x</o:pen>"
        );
        // An unterminated comment drops the rest rather than looping.
        assert_eq!(preprocess_office_html("a<!-- never closed"), "a");
    }

    #[test]
    fn a_perfect_converter_loses_no_words() {
        let mut measure = Measure::default();
        let facts = html_facts("<p>Hello <b>world</b></p>");
        let markdown = evaluate(
            &mut measure,
            &arc(Echo),
            &facts,
            "<p>Hello <b>world</b></p>",
        );
        assert_eq!(markdown.as_deref(), Some("Hello world"));
        assert_eq!(
            (measure.inputs, measure.words_html, measure.words_missing),
            (1, 2, 0)
        );
        assert_eq!(
            (measure.panics, measure.errors, measure.nondeterministic),
            (0, 0, 0)
        );
    }

    #[test]
    fn a_panic_is_counted_not_fatal() {
        let mut measure = Measure::default();
        let facts = html_facts("<p>x</p>");
        assert_eq!(
            evaluate(&mut measure, &arc(Panics), &facts, "<p>x</p>"),
            None
        );
        assert_eq!((measure.panics, measure.errors), (1, 0));
    }

    #[test]
    fn an_error_is_counted() {
        let mut measure = Measure::default();
        let facts = html_facts("<p>x</p>");
        assert_eq!(
            evaluate(&mut measure, &arc(Fails), &facts, "<p>x</p>"),
            None
        );
        assert_eq!((measure.panics, measure.errors), (0, 1));
    }

    #[test]
    fn output_that_changes_between_runs_is_flagged() {
        let mut measure = Measure::default();
        let facts = html_facts("<p>x</p>");
        let _ = evaluate(
            &mut measure,
            &arc(Flaky(AtomicU64::new(0))),
            &facts,
            "<p>x</p>",
        );
        assert_eq!(measure.nondeterministic, 1);
    }

    #[test]
    fn the_synthetic_cases_cover_the_hard_inputs() {
        let cases = synthetic_cases();
        assert_eq!(cases.len(), 16);
        assert!(cases.iter().any(String::is_empty));
        assert!(cases.iter().any(|c| c.contains("mso-")));
        assert!(cases.iter().any(|c| c.contains("cid:")));
        assert!(cases.iter().any(|c| c.matches("<div>").count() == 300));
        assert!(cases.iter().any(|c| c.contains("&eacute;")));
    }

    #[test]
    fn the_run_without_candidates_says_so() {
        // Without the `bakeoff` feature there are no candidates; with it there are.
        let compiled_in = !candidates().is_empty();
        assert_eq!(compiled_in, cfg!(feature = "bakeoff"));
    }

    #[test]
    fn an_output_directory_that_is_not_new_or_empty_is_refused() {
        let base = std::env::temp_dir().join(format!("tsp-bakeoff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        assert!(prepare_output_dir(&base).is_ok());
        fs::write(base.join("keep.txt"), b"mine").unwrap();
        assert!(prepare_output_dir(&base).is_err());
        assert_eq!(fs::read(base.join("keep.txt")).unwrap(), b"mine");
        let _ = fs::remove_dir_all(&base);
    }
}
