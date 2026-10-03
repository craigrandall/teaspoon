//! `--dry-run` (M4b-3): plans an export of a PST or `.msg` input without writing anything and
//! prints content-free counts, the "naming census" that the export ADRs are decided from.
//!
//! Everything printed is a count or a bounded number. Names, subjects, and paths are read into
//! memory to plan, and never printed.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};

use crate::naming::{sanitize_component, utf16_len};
use crate::plan::{plan_export, verify_plan, GateReport, Plan, Policy};
use crate::source_msg::build_msg_tree;
use crate::source_pst::build_pst_tree;

pub(crate) const PROP_SUBJECT: u16 = 0x0037; // PidTagSubject
pub(crate) const PROP_SUBMIT_TIME: u16 = 0x0039; // PidTagClientSubmitTime
pub(crate) const PROP_DELIVERY_TIME: u16 = 0x0E06; // PidTagMessageDeliveryTime
pub(crate) const PROP_INTERNET_MESSAGE_ID: u16 = 0x1035; // PidTagInternetMessageId
pub(crate) const PROP_ATTACH_FILENAME: u16 = 0x3704; // PidTagAttachFilename (short)
pub(crate) const PROP_ATTACH_LONG_FILENAME: u16 = 0x3707; // PidTagAttachLongFilename

/// Content-free facts gathered while reading the source, reported next to the plan counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceCensus {
    pub(crate) open_errors: usize,
    pub(crate) subdirectories_mirrored: usize,
    pub(crate) subject_markers_stripped: usize,
    pub(crate) attachments_without_name: usize,
    pub(crate) internet_id_missing: usize,
    pub(crate) internet_id_duplicate: usize,
    pub(crate) non_mail_items: usize,
    pub(crate) associated_items_skipped: usize,
    pub(crate) folder_name_read_errors: usize,
    pub(crate) attachment_row_read_errors: usize,
    pub(crate) embedded_attachments_not_opened: usize,
    pub(crate) attachment_tables_with_long_name_column: usize,
    pub(crate) folder_identity_nid: usize,
    pub(crate) folder_identity_entry_id: usize,
    pub(crate) folder_identity_unavailable: usize,
}

/// Counts messages without an Internet message ID and messages repeating one already seen.
#[derive(Debug, Default)]
pub(crate) struct InternetIdTracker {
    seen: BTreeSet<String>,
}

impl InternetIdTracker {
    pub(crate) fn note(&mut self, id: Option<&str>, census: &mut SourceCensus) {
        match id.map(str::trim).filter(|s| !s.is_empty()) {
            None => census.internet_id_missing += 1,
            Some(id) => {
                if !self.seen.insert(id.to_string()) {
                    census.internet_id_duplicate += 1;
                }
            }
        }
    }
}

/// `PidTagSubject` may begin with U+0001 followed by one more control character that records the
/// length of the subject prefix. The text after those two characters is the subject (prefix
/// plus normalized subject). Returns the subject and whether a marker was removed.
pub(crate) fn strip_subject_marker(s: &str) -> (&str, bool) {
    let mut chars = s.chars();
    if chars.next() == Some('\u{1}') {
        chars.next();
        (chars.as_str(), true)
    } else {
        (s, false)
    }
}

pub(crate) enum SourceKind {
    Pst,
    Msg,
}

pub(crate) fn classify_source(input: &Path) -> Result<SourceKind> {
    if input.is_dir() {
        return Ok(SourceKind::Msg);
    }
    match input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("pst") => Ok(SourceKind::Pst),
        Some("msg") => Ok(SourceKind::Msg),
        _ => anyhow::bail!(
            "unsupported input: expected a .pst file, a .msg file, or a directory of .msg files"
        ),
    }
}

/// The last component of the input as written: the file name, or the directory name.
///
/// Taken from the raw input string rather than via `Path::file_name`: on Windows `:` is an
/// alternative path separator, so `Path::new("a:b.pst").file_stem()` yields `"b"`, silently
/// dropping the `a:` part. Trailing separators are ignored first, so `C:\mail\msgs\` and
/// `C:\mail\msgs` give the same name (without that, the last "component" would be empty).
pub(crate) fn input_leaf_name(input: &Path) -> String {
    let raw = input.to_string_lossy();
    let trimmed = raw.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

/// Name of the export root directory: the input's file stem (or directory
/// name), sanitized.
///
/// The extension is stripped manually, mirroring `Path::file_stem` semantics: a trailing `.ext`
/// is removed only when the part before the dot is non-empty, so a dotfile like `.pst` is
/// preserved as-is.
pub(crate) fn root_stem(input: &Path) -> String {
    let name = input_leaf_name(input);

    let stem = match name.rsplit_once('.') {
        Some((before, _)) if !before.is_empty() => before,
        _ => name.as_str(),
    };

    sanitize_component(stem, "archive").name
}

/// UTF-16 units in the absolute path `<out>\<stem>`.
pub(crate) fn root_units(out: &Path, stem: &str) -> Result<usize> {
    let absolute = std::path::absolute(out).context("failed to resolve the output path")?;
    Ok(utf16_len(&absolute.join(stem).to_string_lossy()))
}

pub(crate) fn run_dry_run(input: &Path, out: &Path) -> Result<()> {
    let kind = classify_source(input)?;
    let stem = root_stem(input);
    let policy = Policy::new(root_units(out, &stem)?);

    let (tree, census, label) = match kind {
        SourceKind::Pst => {
            let (tree, census) = build_pst_tree(input)?;
            (tree, census, "pst_dry_run")
        }
        SourceKind::Msg => {
            let (tree, census) = build_msg_tree(input)?;
            (tree, census, "msg_dry_run")
        }
    };
    let plan = plan_export(&tree, &policy);
    let gate = verify_plan(&plan, &policy);
    print_dry_run_report(label, &policy, &plan, &gate, &census);
    Ok(())
}

/// Prints the census. Keys are a stable, content-free vocabulary like the other reports.
pub(crate) fn print_dry_run_report(
    label: &str,
    policy: &Policy,
    plan: &Plan,
    gate: &GateReport,
    census: &SourceCensus,
) {
    let c = &plan.counters;
    println!("inventory=privacy_safe");
    println!("input_kind={label}");
    println!("plan_root_units={}", policy.root_units);
    println!("plan_max_path_units_allowed={}", policy.max_path_units);
    println!("plan_entries_total={}", c.entries_total);
    println!("plan_folders={}", c.folders);
    println!("plan_messages={}", c.messages);
    println!("plan_attachment_files={}", c.attachment_files);
    println!("plan_embedded_messages={}", c.embedded_messages);
    println!("plan_names_sanitized={}", c.names_sanitized);
    println!("plan_names_truncated={}", c.names_truncated);
    println!("plan_names_fallback={}", c.names_fallback);
    println!("plan_reserved_name_hits={}", c.reserved_name_hits);
    println!("plan_collision_groups={}", c.collision_groups);
    println!("plan_largest_collision_group={}", c.largest_collision_group);
    println!(
        "plan_collision_groups_100_plus={}",
        c.collision_groups_100_plus
    );
    println!("plan_embedded_depth_capped={}", c.embedded_depth_capped);
    println!("plan_budget_exceeded={}", c.budget_exceeded);
    println!("plan_max_path_units={}", c.max_path_units);
    println!(
        "plan_max_relative_path_units={}",
        c.max_path_units.saturating_sub(policy.root_units)
    );
    println!("plan_max_component_units={}", c.max_component_units);
    println!("plan_max_depth={}", c.max_depth);
    println!(
        "plan_max_entries_in_directory={}",
        c.max_entries_in_directory
    );
    println!("plan_gate_collisions={}", gate.collisions);
    println!("plan_gate_over_budget={}", gate.over_budget);
    println!("plan_gate_invalid_names={}", gate.invalid_names);
    println!("plan_gate_unit_mismatches={}", gate.unit_mismatches);
    println!("plan_gate_violations={}", gate.violations());
    println!("source_open_errors={}", census.open_errors);
    println!(
        "source_subdirectories_mirrored={}",
        census.subdirectories_mirrored
    );
    println!(
        "source_subject_markers_stripped={}",
        census.subject_markers_stripped
    );
    println!(
        "source_attachments_without_name={}",
        census.attachments_without_name
    );
    println!(
        "source_internet_message_id_missing={}",
        census.internet_id_missing
    );
    println!(
        "source_internet_message_id_duplicate={}",
        census.internet_id_duplicate
    );
    println!("source_non_mail_items={}", census.non_mail_items);
    println!(
        "source_associated_items_skipped={}",
        census.associated_items_skipped
    );
    println!(
        "source_folder_name_read_errors={}",
        census.folder_name_read_errors
    );
    println!(
        "source_attachment_row_read_errors={}",
        census.attachment_row_read_errors
    );
    println!(
        "source_embedded_attachments_not_opened={}",
        census.embedded_attachments_not_opened
    );
    println!(
        "source_attachment_tables_with_long_name_column={}",
        census.attachment_tables_with_long_name_column
    );
    println!(
        "folder_identity_nid_available={}",
        census.folder_identity_nid
    );
    println!(
        "folder_identity_entry_id_available={}",
        census.folder_identity_entry_id
    );
    println!(
        "folder_identity_unavailable={}",
        census.folder_identity_unavailable
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use crate::plan::{SourceFolder, SourceMessage};
    use clap::Parser;

    #[test]
    fn the_subject_prefix_marker_is_removed() {
        assert_eq!(
            strip_subject_marker("\u{1}\u{4}RE: Hello"),
            ("RE: Hello", true)
        );
        assert_eq!(strip_subject_marker("Plain"), ("Plain", false));
        assert_eq!(strip_subject_marker("\u{1}"), ("", true));
        assert_eq!(strip_subject_marker(""), ("", false));
        // Only a leading marker counts.
        assert_eq!(strip_subject_marker("a\u{1}b"), ("a\u{1}b", false));
    }

    #[test]
    fn internet_ids_are_counted_missing_and_duplicate() {
        let mut census = SourceCensus::default();
        let mut tracker = InternetIdTracker::default();
        tracker.note(Some("<a@x>"), &mut census);
        tracker.note(Some("<b@x>"), &mut census);
        tracker.note(Some("<a@x>"), &mut census);
        tracker.note(Some("  "), &mut census);
        tracker.note(None, &mut census);
        assert_eq!(census.internet_id_missing, 2);
        assert_eq!(census.internet_id_duplicate, 1);
    }

    #[test]
    fn root_names_come_from_the_input_and_are_sanitized() {
        assert_eq!(root_stem(Path::new("Archive 2019.pst")), "Archive 2019");
        assert_eq!(root_stem(Path::new("a:b.pst")), "a_b");
        assert_eq!(root_stem(Path::new(".pst")), ".pst");
        assert_eq!(root_stem(Path::new("")), "archive");
    }

    #[test]
    fn trailing_separators_do_not_change_the_root_name() {
        assert_eq!(root_stem(Path::new("dir/msgs/")), "msgs");
        assert_eq!(root_stem(Path::new("dir\\msgs\\")), "msgs");
        assert_eq!(root_stem(Path::new("dir/msgs")), "msgs");
        assert_eq!(
            root_stem(Path::new("C:\\mail\\Archive 2019.pst")),
            "Archive 2019"
        );
        assert_eq!(input_leaf_name(Path::new("a/b.msg")), "b.msg");
        assert_eq!(input_leaf_name(Path::new("a\\msgs\\")), "msgs");
        assert_eq!(input_leaf_name(Path::new("")), "");
    }

    #[test]
    fn root_units_count_the_whole_absolute_path() {
        let units = root_units(Path::new("out"), "stem").unwrap();
        assert!(units > utf16_len("out") + 1 + utf16_len("stem"));
    }

    #[test]
    fn the_command_line_accepts_a_dry_run_only_with_out() {
        assert!(Args::try_parse_from(["tsp", "x.pst", "--out", "o", "--dry-run"]).is_ok());
        assert!(Args::try_parse_from(["tsp", "x.pst", "--dry-run"]).is_err());
        assert!(Args::try_parse_from(["tsp", "x.pst", "--out", "o"]).is_ok());
        assert!(
            Args::try_parse_from(["tsp", "x.pst", "--out", "o", "--dry-run", "--verify"]).is_err()
        );
        let args = Args::try_parse_from(["tsp", "x.pst", "--out", "o", "--dry-run"]).unwrap();
        assert!(args.dry_run);
        assert!(args.out.is_some());
    }

    #[test]
    fn the_report_prints_for_a_small_plan_without_panicking() {
        let tree = SourceFolder {
            id: 1,
            name: String::new(),
            folders: vec![],
            messages: vec![SourceMessage {
                id: 2,
                subject: Some("Hello".to_string()),
                time: Some(1),
                attachments: vec![],
            }],
        };
        let policy = Policy::new(30);
        let plan = plan_export(&tree, &policy);
        let gate = verify_plan(&plan, &policy);
        assert_eq!(gate.violations(), 0);
        print_dry_run_report(
            "msg_dry_run",
            &policy,
            &plan,
            &gate,
            &SourceCensus::default(),
        );
    }
}
