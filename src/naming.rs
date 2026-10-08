//! Pure naming rules for the export planner (N, L, and U rules in
//! `docs/plans/m4a-export-rules.md`): sanitizing, UTF-16 measurement, shortening, collision keys,
//! and duplicate suffixes. No I/O and no PST or MSG types.
//!
//! The planner is not wired into the CLI until M4b-3 (`--dry-run`), so its items are unused
//! outside tests for now; the allow below is removed in that stage.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Units reserved at the end of every directory name for a duplicate suffix (` (999)`).
pub(crate) const SUFFIX_RESERVE_UNITS: usize = 6;
/// Upper bound on the reserve when a very large group widens its suffix.
pub(crate) const MAX_SUFFIX_RESERVE_UNITS: usize = 12;
/// Maximum UTF-16 code units in one path component (NTFS, FAT32, exFAT).
pub(crate) const COMPONENT_MAX_UNITS: usize = 255;

/// What happened to a name on its way to a legal on-disk name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NameFlag {
    ReservedCharReplaced,
    ControlRemoved,
    InvisibleRemoved,
    Trimmed,
    DeviceNameMangled,
    EmptyFallback,
    Truncated,
    Suffixed,
    DepthCapped,
    SpecialNameProtected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sanitized {
    pub(crate) name: String,
    pub(crate) flags: BTreeSet<NameFlag>,
}

/// Length in UTF-16 code units, the unit Windows limits are counted in.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn is_reserved_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Characters deleted outright: bidi controls and zero-width characters. (U+2028 and U+2029 are
/// whitespace and become a space instead.)
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200D}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

fn is_reserved_device(stem: &str) -> bool {
    let s = stem.trim_end_matches(' ').to_ascii_uppercase();
    if matches!(
        s.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let chars: Vec<char> = s.chars().collect();
    chars.len() == 4
        && (s.starts_with("COM") || s.starts_with("LPT"))
        && (chars[3].is_ascii_digit() || matches!(chars[3], '¹' | '²' | '³'))
}

/// Rules N2-N8: turns any source string into a legal Windows component name. `fallback` must
/// itself be a legal name; it is used when nothing usable remains.
pub(crate) fn sanitize_component(raw: &str, fallback: &str) -> Sanitized {
    let mut flags = BTreeSet::new();
    let mut step = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_whitespace() {
            step.push(c);
        } else if is_invisible(c) {
            flags.insert(NameFlag::InvisibleRemoved);
        } else if c.is_control() {
            flags.insert(NameFlag::ControlRemoved);
        } else if is_reserved_char(c) {
            flags.insert(NameFlag::ReservedCharReplaced);
            step.push('_');
        } else if is_separator(c) {
            flags.insert(NameFlag::ReservedCharReplaced);
            step.push('-');
        } else {
            step.push(c);
        }
    }
    // Normalizing after deletions keeps the result stable when a deleted character was between
    // a base character and a combining mark.
    let step: String = step.nfc().collect();

    let mut collapsed = String::with_capacity(step.len());
    let mut prev_space = false;
    for c in step.chars() {
        if c.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    let trimmed = collapsed.trim_matches(' ').trim_end_matches(['.', ' ']);
    if trimmed.len() != collapsed.len() {
        flags.insert(NameFlag::Trimmed);
    }
    if trimmed.is_empty() {
        flags.insert(NameFlag::EmptyFallback);
        return Sanitized {
            name: fallback.to_string(),
            flags,
        };
    }

    let (stem, rest) = match trimmed.find('.') {
        Some(i) => trimmed.split_at(i),
        None => (trimmed, ""),
    };
    let name = if is_reserved_device(stem) {
        flags.insert(NameFlag::DeviceNameMangled);
        format!("{}_{}", stem.trim_end_matches(' '), rest)
    } else {
        trimmed.to_string()
    };
    Sanitized { name, flags }
}

/// True when `name` could be written as one component without any further change.
pub(crate) fn is_valid_component(name: &str) -> bool {
    if name.is_empty() || utf16_len(name) > COMPONENT_MAX_UNITS {
        return false;
    }
    if name.ends_with(' ') || name.ends_with('.') || name.contains("  ") {
        return false;
    }
    let bad = |c: char| {
        c.is_control()
            || is_reserved_char(c)
            || is_separator(c)
            || is_invisible(c)
            || (c.is_whitespace() && c != ' ')
    };
    if name.chars().any(bad) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("");
    if is_reserved_device(stem) {
        return false;
    }
    name.nfc().eq(name.chars())
}

/// Rule N6: names that Explorer or other tools treat specially when found in a folder.
pub(crate) fn is_special_attachment_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "desktop.ini" | "thumbs.db" | "autorun.inf") || lower.starts_with("~$")
}

/// Rule N10: splits an attachment name into (stem, extension including its dot). The extension
/// is the text after the last dot when it is 1-16 units with no spaces and the stem is not empty.
pub(crate) fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => {
            let ext = &name[i + 1..];
            let units = utf16_len(ext);
            if (1..=16).contains(&units) && !ext.contains(' ') {
                (&name[..i], &name[i..])
            } else {
                (name, "")
            }
        }
        _ => (name, ""),
    }
}

/// Rule L5: shortens to at most `max_units` UTF-16 units, ending in `…` when anything was cut.
/// Never splits a surrogate pair, and never separates a base character from its combining marks
/// (emoji joiner sequences can still be cut between joined parts).
pub(crate) fn shorten_to(s: &str, max_units: usize) -> String {
    if utf16_len(s) <= max_units {
        return s.to_string();
    }
    if max_units == 0 {
        return String::new();
    }
    let budget = max_units - 1;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut units = 0usize;
    let mut cut = 0usize;
    for (i, (_, c)) in chars.iter().enumerate() {
        let u = c.len_utf16();
        if units + u > budget {
            break;
        }
        units += u;
        cut = i + 1;
    }
    while cut > 0 && cut < chars.len() && is_combining_mark(chars[cut].1) {
        cut -= 1;
    }
    let end = if cut == chars.len() {
        s.len()
    } else {
        chars[cut].0
    };
    let mut out = s[..end].to_string();
    while out.ends_with(' ') || out.ends_with('.') {
        out.pop();
    }
    out.push('…');
    out
}

/// Rule U1: case-insensitive, normalization-insensitive key used to detect collisions.
pub(crate) fn collision_key(name: &str) -> String {
    name.nfc()
        .collect::<String>()
        .to_lowercase()
        .nfc()
        .collect()
}

fn digits(n: usize) -> usize {
    n.to_string().len()
}

/// Rule U4: digits used for a group's suffix numbers (at least two).
pub(crate) fn suffix_width(group_size: usize) -> usize {
    digits(group_size).max(2)
}

pub(crate) fn format_suffix(k: usize, width: usize) -> String {
    format!(" ({k:0width$})")
}

/// Units a suffix of this width occupies.
pub(crate) fn suffix_units(width: usize) -> usize {
    width + 3
}

/// One name competing in a namespace (the children of one directory).
#[derive(Debug, Clone)]
pub(crate) struct NsItem {
    pub(crate) id: u64,
    pub(crate) stem: String,
    pub(crate) ext: String,
    pub(crate) time: Option<i64>,
    pub(crate) order: (u8, u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NsAssigned {
    pub(crate) final_name: String,
    /// `None` for an item that kept its bare name; otherwise its position in the group.
    pub(crate) number: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupInfo {
    pub(crate) size: usize,
    pub(crate) width: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct NsResult {
    /// Parallel to the input items.
    pub(crate) items: Vec<NsAssigned>,
    pub(crate) groups: Vec<GroupInfo>,
    pub(crate) max_suffix_units: usize,
}

/// Kind rank first (folders before messages), then time (missing last), then source order, id.
fn sort_key(i: &NsItem) -> (u8, bool, i64, u64, u64) {
    (
        i.order.0,
        i.time.is_none(),
        i.time.unwrap_or(0),
        i.order.1,
        i.id,
    )
}

/// Rules U2-U5: gives every item a final name that is unique under `collision_key`.
///
/// Natural names claim their keys first. Within a group (same key) items are ordered by
/// (kind, time, order, id), so folders precede messages; the first keeps the bare name and the k-th gets ` (kk)`, skipping numbers
/// whose result is already taken. `reserved` names (for example `folder.json`) occupy their
/// bare name, so a colliding item is always suffixed. Independent of input order.
pub(crate) fn assign_namespace(items: &[NsItem], reserved: &[&str]) -> NsResult {
    let reserved_keys: BTreeSet<String> = reserved.iter().map(|r| collision_key(r)).collect();
    let natural: Vec<String> = items
        .iter()
        .map(|i| format!("{}{}", i.stem, i.ext))
        .collect();
    let keys: Vec<String> = natural.iter().map(|n| collision_key(n)).collect();

    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        groups.entry(k.as_str()).or_default().push(i);
    }
    for members in groups.values_mut() {
        members.sort_by_key(|&i| sort_key(&items[i]));
    }

    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for key in groups.keys() {
        claimed.insert((*key).to_string());
    }
    let mut out: Vec<NsAssigned> = natural
        .iter()
        .map(|n| NsAssigned {
            final_name: n.clone(),
            number: None,
        })
        .collect();

    let mut group_infos = Vec::new();
    let mut max_units = 0usize;
    for (key, members) in &groups {
        let phantom = reserved_keys.contains(*key);
        let size = members.len() + usize::from(phantom);
        if size < 2 {
            continue;
        }
        let start = usize::from(!phantom);
        let needers = &members[start..];
        let mut width = suffix_width(size);
        loop {
            let mut local: BTreeSet<String> = BTreeSet::new();
            let mut results: Vec<(usize, usize, String)> = Vec::new();
            let mut n = 2usize;
            let mut max_n = 0usize;
            for &idx in needers {
                loop {
                    // A stem ending in a space must not leave a double space before the suffix.
                    let cand = format!(
                        "{}{}{}",
                        items[idx].stem.trim_end_matches(' '),
                        format_suffix(n, width),
                        items[idx].ext
                    );
                    let ck = collision_key(&cand);
                    if claimed.contains(&ck) || local.contains(&ck) {
                        n += 1;
                        continue;
                    }
                    local.insert(ck);
                    results.push((idx, n, cand));
                    max_n = max_n.max(n);
                    n += 1;
                    break;
                }
            }
            if digits(max_n) > width {
                width = digits(max_n);
                continue;
            }
            claimed.extend(local);
            for (idx, n, name) in results {
                out[idx] = NsAssigned {
                    final_name: name,
                    number: Some(n),
                };
            }
            break;
        }
        max_units = max_units.max(suffix_units(width));
        group_infos.push(GroupInfo { size, width });
    }
    NsResult {
        items: out,
        groups: group_infos,
        max_suffix_units: max_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn san(raw: &str) -> String {
        sanitize_component(raw, "(no subject)").name
    }

    #[test]
    fn utf16_length_counts_astral_characters_as_two() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("é"), 1);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("a😀b"), 4);
    }

    #[test]
    fn reserved_characters_are_replaced_and_separators_become_hyphens() {
        let s = sanitize_component("a<b>c:d\"e|f?g*h/i\\j", "x");
        assert_eq!(s.name, "a_b_c_d_e_f_g_h-i-j");
        assert!(s.flags.contains(&NameFlag::ReservedCharReplaced));
    }

    #[test]
    fn controls_and_invisible_characters_are_deleted() {
        let s = sanitize_component(
            "a\u{0001}b\u{200B}c\u{202E}d\u{FEFF}e\u{007F}f\u{0085}g",
            "x",
        );
        // U+0085 is whitespace, so it becomes a space; every other listed character is deleted.
        assert_eq!(s.name, "abcdef g");
        assert!(s.flags.contains(&NameFlag::ControlRemoved));
        assert!(s.flags.contains(&NameFlag::InvisibleRemoved));
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(san("  a \t\n b\u{00A0}\u{3000}c  "), "a b c");
        assert_eq!(san("line1\u{2028}line2"), "line1 line2");
    }

    #[test]
    fn trailing_dots_and_spaces_are_removed_but_leading_dots_stay() {
        assert_eq!(san("name. . "), "name");
        assert_eq!(san(".hidden"), ".hidden");
        assert_eq!(san("a.b."), "a.b");
    }

    #[test]
    fn dot_only_and_empty_names_use_the_fallback() {
        let s = sanitize_component("...", "(unnamed folder)");
        assert_eq!(s.name, "(unnamed folder)");
        assert!(s.flags.contains(&NameFlag::EmptyFallback));
        assert_eq!(san(""), "(no subject)");
        assert_eq!(san(" \u{200B} "), "(no subject)");
        assert_eq!(san(".."), "(no subject)");
    }

    #[test]
    fn device_names_are_mangled_with_and_without_extensions() {
        assert_eq!(san("NUL"), "NUL_");
        assert_eq!(san("nul"), "nul_");
        assert_eq!(san("CON.txt"), "CON_.txt");
        assert_eq!(san("aux ."), "aux_");
        assert_eq!(san("NUL .txt"), "NUL_.txt");
        assert_eq!(san("COM1"), "COM1_");
        assert_eq!(san("LPT9.log"), "LPT9_.log");
        assert_eq!(san("COM¹"), "COM¹_");
        assert_eq!(san("CONIN$"), "CONIN$_");
        assert_eq!(san("COM10"), "COM10");
        assert_eq!(san("CONSOLE"), "CONSOLE");
        let s = sanitize_component("NUL", "x");
        assert!(s.flags.contains(&NameFlag::DeviceNameMangled));
    }

    #[test]
    fn unicode_is_normalized_to_nfc() {
        let decomposed = "e\u{0301}";
        assert_eq!(san(decomposed), "\u{00E9}");
        // A deleted zero-width character between a base and a mark must not leave a
        // non-normalized result.
        assert_eq!(san("e\u{200B}\u{0301}"), "\u{00E9}");
    }

    #[test]
    fn valid_component_agrees_with_sanitize_on_examples() {
        for ok in [
            "Budget review",
            "(no subject)",
            "a (02)",
            "NUL_",
            "x…",
            "image001.png",
        ] {
            assert!(is_valid_component(ok), "{ok}");
        }
        for bad in [
            "",
            "a.",
            "a ",
            "a  b",
            "a<b",
            "a/b",
            "NUL",
            "con.txt",
            "a\tb",
            "a\u{200B}b",
            "e\u{0301}",
        ] {
            assert!(!is_valid_component(bad), "{bad:?}");
        }
        assert!(!is_valid_component(&"x".repeat(256)));
        assert!(is_valid_component(&"x".repeat(255)));
    }

    #[test]
    fn special_attachment_names_are_recognized() {
        for s in [
            "desktop.ini",
            "Desktop.INI",
            "THUMBS.DB",
            "autorun.inf",
            "~$Budget.xlsx",
        ] {
            assert!(is_special_attachment_name(s), "{s}");
        }
        for s in ["desktop.ini.txt", "my~$file", "thumbs.dbx", "image001.png"] {
            assert!(!is_special_attachment_name(s), "{s}");
        }
    }

    #[test]
    fn extension_splitting_follows_rule_n10() {
        assert_eq!(split_extension("report.pdf"), ("report", ".pdf"));
        assert_eq!(split_extension("a.b.c"), ("a.b", ".c"));
        assert_eq!(split_extension(".gitignore"), (".gitignore", ""));
        assert_eq!(split_extension("noext"), ("noext", ""));
        assert_eq!(split_extension("x.has space"), ("x.has space", ""));
        assert_eq!(
            split_extension("x.12345678901234567"),
            ("x.12345678901234567", "")
        );
        assert_eq!(
            split_extension("x.1234567890123456"),
            ("x", ".1234567890123456")
        );
    }

    #[test]
    fn shortening_respects_units_surrogates_and_combining_marks() {
        assert_eq!(shorten_to("short", 10), "short");
        assert_eq!(shorten_to("abcdefghij", 6), "abcde…");
        assert_eq!(shorten_to("abcdefghij", 1), "…");
        assert_eq!(shorten_to("abc", 0), "");
        // An astral character must not be split: "ab😀cd" is 6 units; limit 4 leaves budget 3,
        // which fits "ab" (2) but not the emoji (2 more).
        assert_eq!(shorten_to("ab😀cd", 4), "ab…");
        // A base character is not separated from its combining mark.
        assert_eq!(shorten_to("abce\u{0301}fgh", 5), "abc…");
        // Trailing spaces and dots are dropped before the ellipsis.
        assert_eq!(shorten_to("ab. . cdef", 6), "ab…");
        for s in [
            "abcdefghij",
            "ab😀😀😀cd",
            "e\u{0301}e\u{0301}e\u{0301}e\u{0301}",
        ] {
            for n in 1..12 {
                assert!(utf16_len(&shorten_to(s, n)) <= n, "{s:?} {n}");
            }
        }
    }

    #[test]
    fn collision_keys_fold_case_and_normalization() {
        assert_eq!(collision_key("Budget"), collision_key("bUdGeT"));
        assert_eq!(collision_key("\u{00E9}"), collision_key("e\u{0301}"));
        assert_ne!(collision_key("a"), collision_key("b"));
    }

    #[test]
    fn suffix_width_and_format_follow_rule_u4() {
        assert_eq!(suffix_width(2), 2);
        assert_eq!(suffix_width(99), 2);
        assert_eq!(suffix_width(100), 3);
        assert_eq!(suffix_width(1500), 4);
        assert_eq!(format_suffix(2, 2), " (02)");
        assert_eq!(format_suffix(30, 2), " (30)");
        assert_eq!(format_suffix(2, 4), " (0002)");
        assert_eq!(suffix_units(2), 5);
        assert_eq!(suffix_units(3), 6);
    }

    fn item(id: u64, stem: &str, ext: &str, time: Option<i64>) -> NsItem {
        NsItem {
            id,
            stem: stem.to_string(),
            ext: ext.to_string(),
            time,
            order: (1, id),
        }
    }

    fn names(r: &NsResult) -> Vec<String> {
        r.items.iter().map(|i| i.final_name.clone()).collect()
    }

    #[test]
    fn unique_names_are_untouched() {
        let r = assign_namespace(&[item(1, "a", "", None), item(2, "b", "", None)], &[]);
        assert_eq!(names(&r), vec!["a", "b"]);
        assert!(r.groups.is_empty());
        assert_eq!(r.max_suffix_units, 0);
    }

    #[test]
    fn duplicates_are_numbered_by_position_in_time_order() {
        let items = [
            item(1, "Budget", "", Some(300)),
            item(2, "Budget", "", Some(100)),
            item(3, "Budget", "", Some(200)),
        ];
        let r = assign_namespace(&items, &[]);
        // Earliest (id 2) keeps the bare name; id 3 is second; id 1 is third.
        assert_eq!(names(&r), vec!["Budget (03)", "Budget", "Budget (02)"]);
        assert_eq!(r.items[1].number, None);
        assert_eq!(r.items[2].number, Some(2));
        assert_eq!(r.groups, vec![GroupInfo { size: 3, width: 2 }]);
    }

    #[test]
    fn missing_times_sort_last_and_ties_use_order_then_id() {
        let items = [
            item(5, "x", "", None),
            item(6, "x", "", Some(1)),
            item(4, "x", "", None),
        ];
        let r = assign_namespace(&items, &[]);
        assert_eq!(names(&r), vec!["x (03)", "x", "x (02)"]);
    }

    #[test]
    fn case_and_normalization_variants_collide() {
        let items = [
            item(1, "Inbox", "", Some(1)),
            item(2, "inbox", "", Some(2)),
            item(3, "caf\u{00E9}", "", Some(3)),
            item(4, "cafe\u{0301}", "", Some(4)),
        ];
        let r = assign_namespace(&items, &[]);
        assert_eq!(
            names(&r),
            vec!["Inbox", "inbox (02)", "caf\u{00E9}", "cafe\u{0301} (02)"]
        );
    }

    #[test]
    fn a_stem_ending_in_a_space_does_not_double_the_space() {
        let items = [
            item(1, "a ", ".pdf", Some(1)),
            item(2, "a ", ".pdf", Some(2)),
        ];
        let r = assign_namespace(&items, &[]);
        assert_eq!(names(&r), vec!["a .pdf", "a (02).pdf"]);
    }

    #[test]
    fn folders_precede_messages_in_a_mixed_group() {
        let folder = NsItem {
            id: 1,
            stem: "Budget".into(),
            ext: String::new(),
            time: None,
            order: (0, 1),
        };
        let message = NsItem {
            id: 2,
            stem: "budget".into(),
            ext: String::new(),
            time: Some(1),
            order: (1, 2),
        };
        let r = assign_namespace(&[message, folder], &[]);
        assert_eq!(names(&r), vec!["budget (02)", "Budget"]);
    }

    #[test]
    fn extensions_stay_last_and_count_in_the_key() {
        let items = [
            item(1, "image001", ".png", None),
            item(2, "image001", ".png", None),
            item(3, "image001", ".jpg", None),
        ];
        let r = assign_namespace(&items, &[]);
        assert_eq!(
            names(&r),
            vec!["image001.png", "image001 (02).png", "image001.jpg"]
        );
    }

    #[test]
    fn a_natural_name_that_looks_like_a_suffix_is_never_displaced() {
        let items = [
            item(1, "Budget", "", Some(1)),
            item(2, "Budget", "", Some(2)),
            item(3, "Budget (02)", "", Some(3)),
        ];
        let r = assign_namespace(&items, &[]);
        // The real "Budget (02)" keeps its name; the second "Budget" skips to (03).
        assert_eq!(names(&r), vec!["Budget", "Budget (03)", "Budget (02)"]);
    }

    #[test]
    fn reserved_names_are_always_suffixed() {
        let items = [
            item(1, "folder.json", "", None),
            item(2, "FOLDER.JSON", "", None),
        ];
        let r = assign_namespace(&items, &["folder.json"]);
        assert_eq!(names(&r), vec!["folder.json (02)", "FOLDER.JSON (03)"]);
        let r = assign_namespace(&[item(1, "other", "", None)], &["folder.json"]);
        assert_eq!(names(&r), vec!["other"]);
    }

    #[test]
    fn suffix_width_grows_with_the_group_and_sorts_in_order() {
        for n in [2usize, 9, 10, 99, 100, 101, 999, 1000, 1500] {
            let items: Vec<NsItem> = (0..n as u64)
                .map(|i| item(i, "(no subject)", "", Some(i as i64)))
                .collect();
            let r = assign_namespace(&items, &[]);
            let got = names(&r);
            let expected_width = suffix_width(n);
            assert_eq!(
                r.groups,
                vec![GroupInfo {
                    size: n,
                    width: expected_width
                }]
            );
            assert_eq!(got[0], "(no subject)");
            assert_eq!(
                got[n - 1],
                format!("(no subject){}", format_suffix(n, expected_width))
            );
            // Plain text order equals chronological order within the group.
            let mut sorted = got.clone();
            sorted.sort();
            assert_eq!(sorted, got, "n={n}");
            // And all keys are unique.
            let keys: BTreeSet<String> = got.iter().map(|g| collision_key(g)).collect();
            assert_eq!(keys.len(), n);
        }
    }

    #[test]
    fn assignment_does_not_depend_on_input_order() {
        let items = vec![
            item(1, "a", "", Some(5)),
            item(2, "a", "", Some(1)),
            item(3, "A", "", None),
            item(4, "b", "", Some(2)),
            item(5, "a (02)", "", Some(9)),
        ];
        let forward = assign_namespace(&items, &[]);
        let mut rev = items.clone();
        rev.reverse();
        let backward = assign_namespace(&rev, &[]);
        let f: BTreeMap<u64, String> = items
            .iter()
            .zip(forward.items.iter())
            .map(|(i, a)| (i.id, a.final_name.clone()))
            .collect();
        let b: BTreeMap<u64, String> = rev
            .iter()
            .zip(backward.items.iter())
            .map(|(i, a)| (i.id, a.final_name.clone()))
            .collect();
        assert_eq!(f, b);
    }

    fn nasty_char() -> impl Strategy<Value = char> {
        prop_oneof![
            any::<char>(),
            prop::sample::select(vec![
                '<',
                '>',
                ':',
                '"',
                '|',
                '?',
                '*',
                '/',
                '\\',
                '.',
                ' ',
                '\u{200B}',
                '\u{202E}',
                '\u{0301}',
                '\u{00A0}',
                '\t',
                '\u{1F600}',
                'é',
                'e',
                'N',
                'U',
                'L',
                'C',
                'O',
                '1',
            ]),
        ]
    }

    fn nasty_string() -> impl Strategy<Value = String> {
        prop::collection::vec(nasty_char(), 0..40).prop_map(|v| v.into_iter().collect())
    }

    proptest! {
        #[test]
        fn sanitize_is_idempotent_and_always_valid(raw in nasty_string()) {
            let once = sanitize_component(&raw, "(no subject)").name;
            let twice = sanitize_component(&once, "(no subject)").name;
            prop_assert_eq!(&once, &twice);
            prop_assert!(is_valid_component(&once), "{:?}", once);
        }

        #[test]
        fn shorten_never_exceeds_the_limit(raw in nasty_string(), limit in 1usize..60) {
            let s = sanitize_component(&raw, "(no subject)").name;
            let short = shorten_to(&s, limit);
            prop_assert!(utf16_len(&short) <= limit);
            if utf16_len(&s) <= limit {
                prop_assert_eq!(&short, &s);
            } else {
                prop_assert!(short.ends_with('…'));
                prop_assert!(!short.ends_with(" …") && !short.ends_with(".…"));
            }
        }

        #[test]
        fn collision_key_is_case_insensitive_for_ascii(s in "[ -~]{0,30}") {
            prop_assert_eq!(collision_key(&s.to_uppercase()), collision_key(&s.to_lowercase()));
        }

        #[test]
        fn assignment_is_unique_deterministic_and_order_independent(
            specs in prop::collection::vec(
                (prop::sample::select(vec!["a", "A", "b", "a (02)", "a (03)", "folder.json", "é", "e\u{0301}"]),
                 prop::option::of(0i64..5)),
                0..40),
        ) {
            let items: Vec<NsItem> = specs
                .iter()
                .enumerate()
                .map(|(i, (stem, t))| NsItem {
                    id: i as u64,
                    stem: (*stem).to_string(),
                    ext: String::new(),
                    time: *t,
                    order: (1, i as u64),
                })
                .collect();
            let r = assign_namespace(&items, &["folder.json"]);
            let mut seen = BTreeSet::new();
            seen.insert(collision_key("folder.json"));
            for a in &r.items {
                prop_assert!(seen.insert(collision_key(&a.final_name)), "duplicate {:?}", a.final_name);
            }
            let mut rev = items.clone();
            rev.reverse();
            let r2 = assign_namespace(&rev, &["folder.json"]);
            let f: BTreeMap<u64, String> = items.iter().zip(r.items.iter())
                .map(|(i, a)| (i.id, a.final_name.clone())).collect();
            let b: BTreeMap<u64, String> = rev.iter().zip(r2.items.iter())
                .map(|(i, a)| (i.id, a.final_name.clone())).collect();
            prop_assert_eq!(f, b);
        }
    }
}
