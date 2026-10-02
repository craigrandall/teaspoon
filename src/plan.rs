//! Pure export planner (rules L1-L5 and U1-U9 in `docs/plans/m4a-export-rules.md`): given a
//! source tree of names, times, and identifiers, produces every final directory and file name,
//! each checked against the Windows path budget, plus content-free counters. No I/O.
//!
//! Layout planned: `<root>/<folder>.../<message>/{message.md,metadata.json,attachments/...}`.
//! The planner is not wired into the CLI until M4b-3 (`--dry-run`), so its items are unused
//! outside tests for now; the allow below is removed in that stage.
//!
//! Known limit, to be closed before the PST export ships: when even the floor names cannot fit
//! (very deep folder chains on a long root), the planner reports `budget_exceeded` instead of
//! flattening folder chains (rule L4 step 4) or falling back to identity names (step 5).
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use crate::naming::{
    assign_namespace, collision_key, is_special_attachment_name, is_valid_component,
    sanitize_component, shorten_to, split_extension, utf16_len, NameFlag, NsItem, Sanitized,
    COMPONENT_MAX_UNITS, MAX_SUFFIX_RESERVE_UNITS, SUFFIX_RESERVE_UNITS,
};

/// Name reserved in every folder directory for that folder's metadata file.
pub(crate) const FOLDER_JSON: &str = "folder.json";
const FOLDER_JSON_TAIL: usize = 1 + 11; // "\folder.json"
const METADATA_JSON_TAIL: usize = 1 + 13; // "\metadata.json"
const ATTACHMENTS_DIR_UNITS: usize = 11; // "attachments"
const ATTACHMENTS_TAIL: usize = 1 + ATTACHMENTS_DIR_UNITS + 1; // "\attachments\"

/// Shortest stems the planner will cut to (including the ellipsis), by item kind.
const FOLDER_FLOOR: usize = 16;
const MESSAGE_FLOOR: usize = 24;
const ATTACH_FLOOR: usize = 16;

/// Limits the plan must respect.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    /// UTF-16 units in the absolute path of the export root (`<out>\<stem>`).
    pub(crate) root_units: usize,
    /// Longest allowed full path (259 leaves room for the terminating null under MAX_PATH).
    pub(crate) max_path_units: usize,
    /// Longest allowed directory path (reportedly 248 for `CreateDirectory`, so 247).
    pub(crate) dir_max_units: usize,
    /// Longest allowed attachment file name.
    pub(crate) attachment_cap_units: usize,
    /// How many levels of embedded messages are written as nested folders.
    pub(crate) max_embedded_depth: usize,
}

impl Policy {
    pub(crate) fn new(root_units: usize) -> Self {
        Policy {
            root_units,
            max_path_units: 259,
            dir_max_units: 247,
            attachment_cap_units: 100,
            max_embedded_depth: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourceFolder {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) folders: Vec<SourceFolder>,
    pub(crate) messages: Vec<SourceMessage>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceMessage {
    pub(crate) id: u64,
    pub(crate) subject: Option<String>,
    /// Delivery or submit time, in any monotonic unit; `None` sorts last.
    pub(crate) time: Option<i64>,
    pub(crate) attachments: Vec<SourceAttachment>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceAttachment {
    pub(crate) id: u64,
    pub(crate) name: Option<String>,
    /// Present when the attachment is itself a message.
    pub(crate) embedded: Option<SourceMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntryKind {
    Folder,
    Message,
    AttachmentFile,
    EmbeddedMessage,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanEntry {
    pub(crate) kind: EntryKind,
    pub(crate) source_id: u64,
    /// Path components below the export root, last component is this entry's own name.
    pub(crate) components: Vec<String>,
    /// UTF-16 units in this entry's absolute path.
    pub(crate) path_units: usize,
    /// Longest absolute path that will exist at or below this entry's fixed children.
    pub(crate) worst_units: usize,
    pub(crate) flags: BTreeSet<NameFlag>,
    /// The source name, before any change (may be empty).
    pub(crate) original: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlanCounters {
    pub(crate) entries_total: usize,
    pub(crate) folders: usize,
    pub(crate) messages: usize,
    pub(crate) attachment_files: usize,
    pub(crate) embedded_messages: usize,
    pub(crate) names_sanitized: usize,
    pub(crate) names_truncated: usize,
    pub(crate) names_fallback: usize,
    pub(crate) reserved_name_hits: usize,
    pub(crate) collision_groups: usize,
    pub(crate) largest_collision_group: usize,
    pub(crate) collision_groups_100_plus: usize,
    pub(crate) embedded_depth_capped: usize,
    pub(crate) budget_exceeded: usize,
    pub(crate) max_path_units: usize,
    pub(crate) max_component_units: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_entries_in_directory: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    /// Sorted by components, so the order never depends on the order of the source tree.
    pub(crate) entries: Vec<PlanEntry>,
    pub(crate) counters: PlanCounters,
}

fn folder_natural(raw: &str) -> Sanitized {
    sanitize_component(raw, "(unnamed folder)")
}

fn message_natural(raw: Option<&str>) -> Sanitized {
    sanitize_component(raw.unwrap_or(""), "(no subject)")
}

fn file_natural(raw: Option<&str>, position: usize) -> (Sanitized, String, String) {
    let fallback = format!("attachment-{position}");
    let mut s = sanitize_component(raw.unwrap_or(""), &fallback);
    if is_special_attachment_name(&s.name) {
        s.name = format!("_{}", s.name);
        s.flags.insert(NameFlag::SpecialNameProtected);
    }
    let (stem, ext) = {
        let (a, b) = split_extension(&s.name);
        (a.to_string(), b.to_string())
    };
    (s, stem, ext)
}

/// Smallest length a directory name can be cut to, plus the suffix reserve.
fn dir_floor_units(natural: &str, floor: usize) -> usize {
    utf16_len(natural).min(floor) + SUFFIX_RESERVE_UNITS
}

fn file_floor_units(stem: &str, ext: &str) -> usize {
    utf16_len(stem).min(ATTACH_FLOOR) + utf16_len(ext) + SUFFIX_RESERVE_UNITS
}

fn entry_over_budget(e: &PlanEntry, policy: &Policy) -> bool {
    let last = e.components.last().map_or(0, |c| utf16_len(c));
    let is_dir = e.kind != EntryKind::AttachmentFile;
    last > COMPONENT_MAX_UNITS
        || e.worst_units > policy.max_path_units
        || (is_dir && e.path_units > policy.dir_max_units)
}

struct Cand {
    id: u64,
    natural: Sanitized,
    stem: String,
    ext: String,
    time: Option<i64>,
    order: (u8, u64),
    /// Most units the full component (stem, suffix, extension) may occupy.
    limit_total: usize,
    floor_stem: usize,
}

struct Assigned {
    final_name: String,
    flags: BTreeSet<NameFlag>,
}

struct Planner<'a> {
    policy: &'a Policy,
    entries: Vec<PlanEntry>,
    counters: PlanCounters,
}

impl Planner<'_> {
    /// Largest component length for a directory whose parent path has `parent_units` and whose
    /// descendants need at least `tail` more units.
    fn dir_limit_total(&self, parent_units: usize, tail: usize) -> usize {
        let hard =
            COMPONENT_MAX_UNITS.min(self.policy.dir_max_units.saturating_sub(parent_units + 1));
        let by_tail = self
            .policy
            .max_path_units
            .saturating_sub(parent_units + 1 + tail);
        hard.min(by_tail)
    }

    /// What the attachments of `m` need below `attachments\`, with every name at its floor.
    fn attachments_need(&self, m: &SourceMessage, depth: usize) -> usize {
        let mut need = 0usize;
        for (i, a) in m.attachments.iter().enumerate() {
            let n = match &a.embedded {
                Some(e) => {
                    let natural = message_natural(e.subject.as_deref()).name;
                    let inner = if depth < self.policy.max_embedded_depth {
                        self.message_tail(e, depth + 1)
                    } else {
                        METADATA_JSON_TAIL
                    };
                    dir_floor_units(&natural, MESSAGE_FLOOR) + inner
                }
                None => {
                    let (_, stem, ext) = file_natural(a.name.as_deref(), i + 1);
                    file_floor_units(&stem, &ext)
                }
            };
            need = need.max(n);
        }
        need
    }

    /// Longest path below a message directory, with attachments at their floors.
    fn message_tail(&self, m: &SourceMessage, depth: usize) -> usize {
        if m.attachments.is_empty() {
            METADATA_JSON_TAIL
        } else {
            METADATA_JSON_TAIL.max(ATTACHMENTS_TAIL + self.attachments_need(m, depth))
        }
    }

    /// Longest path below a folder directory, with every descendant at its floor.
    fn folder_tail(&self, f: &SourceFolder) -> usize {
        let mut t = FOLDER_JSON_TAIL;
        for s in &f.folders {
            let n = folder_natural(&s.name).name;
            t = t.max(1 + dir_floor_units(&n, FOLDER_FLOOR) + self.folder_tail(s));
        }
        for m in &f.messages {
            let n = message_natural(m.subject.as_deref()).name;
            t = t.max(1 + dir_floor_units(&n, MESSAGE_FLOOR) + self.message_tail(m, 0));
        }
        t
    }

    /// Rules U2-U5 plus length limits: shortens every candidate, then assigns unique names.
    /// If a very large group needs a wider suffix, everything is shortened a little more and
    /// assigned again.
    fn assign_cands(&mut self, cands: &[Cand], reserved: &[&str]) -> Vec<Assigned> {
        let mut reserve = SUFFIX_RESERVE_UNITS;
        loop {
            let mut items = Vec::with_capacity(cands.len());
            let mut shortened = Vec::with_capacity(cands.len());
            for c in cands {
                let ext_units = utf16_len(&c.ext);
                let stem_units = utf16_len(&c.stem);
                let limit = c
                    .limit_total
                    .saturating_sub(reserve + ext_units)
                    .max(c.floor_stem.min(stem_units));
                let stem = shorten_to(&c.stem, limit);
                shortened.push(stem != c.stem);
                items.push(NsItem {
                    id: c.id,
                    stem,
                    ext: c.ext.clone(),
                    time: c.time,
                    order: c.order,
                });
            }
            let res = assign_namespace(&items, reserved);
            if res.max_suffix_units > reserve && reserve < MAX_SUFFIX_RESERVE_UNITS {
                reserve = res.max_suffix_units.min(MAX_SUFFIX_RESERVE_UNITS);
                continue;
            }
            for g in &res.groups {
                self.counters.collision_groups += 1;
                self.counters.largest_collision_group =
                    self.counters.largest_collision_group.max(g.size);
                if g.size >= 100 {
                    self.counters.collision_groups_100_plus += 1;
                }
            }
            self.counters.max_entries_in_directory = self
                .counters
                .max_entries_in_directory
                .max(cands.len() + reserved.len());
            return cands
                .iter()
                .zip(res.items.iter())
                .zip(shortened.iter())
                .map(|((c, a), &cut)| {
                    let mut flags = c.natural.flags.clone();
                    if cut {
                        flags.insert(NameFlag::Truncated);
                    }
                    if a.number.is_some() {
                        flags.insert(NameFlag::Suffixed);
                    }
                    Assigned {
                        final_name: a.final_name.clone(),
                        flags,
                    }
                })
                .collect();
        }
    }

    fn push_entry(&mut self, entry: PlanEntry) {
        let over = entry_over_budget(&entry, self.policy);
        let c = &mut self.counters;
        c.entries_total += 1;
        match entry.kind {
            EntryKind::Folder => c.folders += 1,
            EntryKind::Message => c.messages += 1,
            EntryKind::AttachmentFile => c.attachment_files += 1,
            EntryKind::EmbeddedMessage => c.embedded_messages += 1,
        }
        let sanitized = entry.flags.iter().any(|f| {
            matches!(
                f,
                NameFlag::ReservedCharReplaced
                    | NameFlag::ControlRemoved
                    | NameFlag::InvisibleRemoved
                    | NameFlag::Trimmed
                    | NameFlag::DeviceNameMangled
                    | NameFlag::SpecialNameProtected
            )
        });
        if sanitized {
            c.names_sanitized += 1;
        }
        if entry.flags.contains(&NameFlag::EmptyFallback) {
            c.names_fallback += 1;
        }
        if entry.flags.contains(&NameFlag::Truncated) {
            c.names_truncated += 1;
        }
        if entry.flags.contains(&NameFlag::DeviceNameMangled) {
            c.reserved_name_hits += 1;
        }
        if over {
            c.budget_exceeded += 1;
        }
        c.max_path_units = c.max_path_units.max(entry.worst_units);
        c.max_component_units = c
            .max_component_units
            .max(entry.components.last().map_or(0, |n| utf16_len(n)));
        c.max_depth = c.max_depth.max(entry.components.len());
        self.entries.push(entry);
    }

    fn walk_folder(&mut self, f: &SourceFolder, comps: &[String], dir_units: usize) {
        let mut cands: Vec<Cand> = Vec::with_capacity(f.folders.len() + f.messages.len());
        for s in &f.folders {
            let natural = folder_natural(&s.name);
            let tail = self.folder_tail(s);
            cands.push(Cand {
                id: cands.len() as u64,
                stem: natural.name.clone(),
                natural,
                ext: String::new(),
                time: None,
                order: (0, s.id),
                limit_total: self.dir_limit_total(dir_units, tail),
                floor_stem: FOLDER_FLOOR,
            });
        }
        for m in &f.messages {
            let natural = message_natural(m.subject.as_deref());
            let tail = self.message_tail(m, 0);
            cands.push(Cand {
                id: cands.len() as u64,
                stem: natural.name.clone(),
                natural,
                ext: String::new(),
                time: m.time,
                order: (1, m.id),
                limit_total: self.dir_limit_total(dir_units, tail),
                floor_stem: MESSAGE_FLOOR,
            });
        }
        let assigned = self.assign_cands(&cands, &[FOLDER_JSON]);
        let nf = f.folders.len();
        for (i, s) in f.folders.iter().enumerate() {
            let a = &assigned[i];
            let units = dir_units + 1 + utf16_len(&a.final_name);
            let mut c = comps.to_vec();
            c.push(a.final_name.clone());
            self.push_entry(PlanEntry {
                kind: EntryKind::Folder,
                source_id: s.id,
                components: c.clone(),
                path_units: units,
                worst_units: units + FOLDER_JSON_TAIL,
                flags: a.flags.clone(),
                original: s.name.clone(),
            });
            self.walk_folder(s, &c, units);
        }
        for (j, m) in f.messages.iter().enumerate() {
            let a = &assigned[nf + j];
            let units = dir_units + 1 + utf16_len(&a.final_name);
            let mut c = comps.to_vec();
            c.push(a.final_name.clone());
            self.push_entry(PlanEntry {
                kind: EntryKind::Message,
                source_id: m.id,
                components: c.clone(),
                path_units: units,
                worst_units: units + METADATA_JSON_TAIL,
                flags: a.flags.clone(),
                original: m.subject.clone().unwrap_or_default(),
            });
            self.walk_message(m, &c, units, 0);
        }
    }

    /// Plans the contents of `<message dir>\attachments`. `depth` is how many embedded levels
    /// enclose `m` (0 for a message in a folder).
    fn walk_message(
        &mut self,
        m: &SourceMessage,
        comps: &[String],
        dir_units: usize,
        depth: usize,
    ) {
        if m.attachments.is_empty() {
            return;
        }
        let att_units = dir_units + 1 + ATTACHMENTS_DIR_UNITS;
        let room = self
            .policy
            .max_path_units
            .saturating_sub(dir_units + ATTACHMENTS_TAIL);
        let cap = self
            .policy
            .attachment_cap_units
            .min(room)
            .min(COMPONENT_MAX_UNITS);
        let mut cands: Vec<Cand> = Vec::with_capacity(m.attachments.len());
        for (i, a) in m.attachments.iter().enumerate() {
            let position = i as u64;
            match &a.embedded {
                None => {
                    let (natural, stem, ext) = file_natural(a.name.as_deref(), i + 1);
                    cands.push(Cand {
                        id: position,
                        natural,
                        stem,
                        ext,
                        time: None,
                        order: (0, position),
                        limit_total: cap,
                        floor_stem: ATTACH_FLOOR,
                    });
                }
                Some(e) => {
                    let natural = message_natural(e.subject.as_deref());
                    let tail = if depth < self.policy.max_embedded_depth {
                        self.message_tail(e, depth + 1)
                    } else {
                        METADATA_JSON_TAIL
                    };
                    cands.push(Cand {
                        id: position,
                        stem: natural.name.clone(),
                        natural,
                        ext: String::new(),
                        time: None,
                        order: (0, position),
                        limit_total: self.dir_limit_total(att_units, tail),
                        floor_stem: MESSAGE_FLOOR,
                    });
                }
            }
        }
        let assigned = self.assign_cands(&cands, &[]);
        for (i, a) in m.attachments.iter().enumerate() {
            let named = &assigned[i];
            let units = att_units + 1 + utf16_len(&named.final_name);
            let mut c = comps.to_vec();
            c.push("attachments".to_string());
            c.push(named.final_name.clone());
            match &a.embedded {
                None => self.push_entry(PlanEntry {
                    kind: EntryKind::AttachmentFile,
                    source_id: a.id,
                    components: c,
                    path_units: units,
                    worst_units: units,
                    flags: named.flags.clone(),
                    original: a.name.clone().unwrap_or_default(),
                }),
                Some(e) => {
                    let mut flags = named.flags.clone();
                    let capped = depth >= self.policy.max_embedded_depth;
                    if capped {
                        flags.insert(NameFlag::DepthCapped);
                        self.counters.embedded_depth_capped += 1;
                    }
                    self.push_entry(PlanEntry {
                        kind: EntryKind::EmbeddedMessage,
                        source_id: e.id,
                        components: c.clone(),
                        path_units: units,
                        worst_units: units + METADATA_JSON_TAIL,
                        flags,
                        original: e.subject.clone().unwrap_or_default(),
                    });
                    if !capped {
                        self.walk_message(e, &c, units, depth + 1);
                    }
                }
            }
        }
    }
}

/// Plans an export. `root`'s own name is not used: its children are planned directly below the
/// export root, whose absolute length is `policy.root_units`.
pub(crate) fn plan_export(root: &SourceFolder, policy: &Policy) -> Plan {
    let mut planner = Planner {
        policy,
        entries: Vec::new(),
        counters: PlanCounters::default(),
    };
    planner.walk_folder(root, &[], policy.root_units);
    let Planner {
        mut entries,
        counters,
        ..
    } = planner;
    entries.sort_by(|a, b| a.components.cmp(&b.components));
    Plan { entries, counters }
}

/// Result of re-checking a finished plan from scratch (rule U9 and the budgets).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GateReport {
    pub(crate) collisions: usize,
    pub(crate) over_budget: usize,
    pub(crate) invalid_names: usize,
    pub(crate) unit_mismatches: usize,
}

impl GateReport {
    pub(crate) fn violations(&self) -> usize {
        self.collisions + self.over_budget + self.invalid_names + self.unit_mismatches
    }
}

/// Re-derives everything the plan promises without trusting the planner: every directory's
/// names are unique under the case-insensitive key (with `folder.json` reserved in folder
/// directories), every name is legal, every path is within budget, and every recorded length
/// matches the components.
pub(crate) fn verify_plan(plan: &Plan, policy: &Policy) -> GateReport {
    let mut report = GateReport::default();
    let mut spaces: BTreeMap<(bool, Vec<String>), Vec<String>> = BTreeMap::new();
    for e in &plan.entries {
        let Some((last, parent)) = e.components.split_last() else {
            continue;
        };
        let folder_namespace = matches!(e.kind, EntryKind::Folder | EntryKind::Message);
        spaces
            .entry((folder_namespace, parent.to_vec()))
            .or_default()
            .push(last.clone());
        if !is_valid_component(last) {
            report.invalid_names += 1;
        }
        if entry_over_budget(e, policy) {
            report.over_budget += 1;
        }
        let units =
            policy.root_units + e.components.iter().map(|c| 1 + utf16_len(c)).sum::<usize>();
        if units != e.path_units {
            report.unit_mismatches += 1;
        }
    }
    for ((folder_namespace, _), names) in &spaces {
        let mut keys = BTreeSet::new();
        if *folder_namespace {
            keys.insert(collision_key(FOLDER_JSON));
        }
        for n in names {
            if !keys.insert(collision_key(n)) {
                report.collisions += 1;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn folder(
        id: u64,
        name: &str,
        folders: Vec<SourceFolder>,
        messages: Vec<SourceMessage>,
    ) -> SourceFolder {
        SourceFolder {
            id,
            name: name.to_string(),
            folders,
            messages,
        }
    }

    fn msg(
        id: u64,
        subject: &str,
        time: Option<i64>,
        attachments: Vec<SourceAttachment>,
    ) -> SourceMessage {
        SourceMessage {
            id,
            subject: Some(subject.to_string()),
            time,
            attachments,
        }
    }

    fn file(id: u64, name: &str) -> SourceAttachment {
        SourceAttachment {
            id,
            name: Some(name.to_string()),
            embedded: None,
        }
    }

    fn embedded(id: u64, m: SourceMessage) -> SourceAttachment {
        SourceAttachment {
            id,
            name: None,
            embedded: Some(m),
        }
    }

    fn paths(plan: &Plan) -> Vec<String> {
        plan.entries
            .iter()
            .map(|e| e.components.join("/"))
            .collect()
    }

    fn clean(plan: &Plan, policy: &Policy) {
        let report = verify_plan(plan, policy);
        assert_eq!(report, GateReport::default(), "{:?}", paths(plan));
        assert_eq!(plan.counters.budget_exceeded, 0);
    }

    #[test]
    fn a_simple_tree_maps_to_the_documented_layout() {
        let root = folder(
            0,
            "root",
            vec![folder(
                1,
                "Inbox",
                vec![],
                vec![msg(
                    10,
                    "Budget review",
                    Some(5),
                    vec![file(100, "Q3 plan.xlsx"), file(101, "image001.png")],
                )],
            )],
            vec![],
        );
        let policy = Policy::new(40);
        let plan = plan_export(&root, &policy);
        assert_eq!(
            paths(&plan),
            vec![
                "Inbox",
                "Inbox/Budget review",
                "Inbox/Budget review/attachments/Q3 plan.xlsx",
                "Inbox/Budget review/attachments/image001.png",
            ]
        );
        clean(&plan, &policy);
        assert_eq!(plan.counters.folders, 1);
        assert_eq!(plan.counters.messages, 1);
        assert_eq!(plan.counters.attachment_files, 2);
        assert_eq!(plan.counters.entries_total, 4);
        // 40 + \Inbox (6) + \Budget review (14) + \attachments (12) + \Q3 plan.xlsx (13) = 85
        assert_eq!(plan.counters.max_path_units, 85);
    }

    #[test]
    fn same_subject_messages_are_numbered_in_time_order() {
        let root = folder(
            0,
            "root",
            vec![],
            vec![
                msg(1, "RE: Meeting", Some(30), vec![]),
                msg(2, "RE: Meeting", Some(10), vec![]),
                msg(3, "RE: Meeting", Some(20), vec![]),
            ],
        );
        let policy = Policy::new(20);
        let plan = plan_export(&root, &policy);
        assert_eq!(
            paths(&plan),
            vec!["RE_ Meeting", "RE_ Meeting (02)", "RE_ Meeting (03)"]
        );
        let by_id = |id: u64| {
            plan.entries
                .iter()
                .find(|e| e.source_id == id)
                .map(|e| e.components[0].clone())
                .unwrap()
        };
        assert_eq!(by_id(2), "RE_ Meeting");
        assert_eq!(by_id(3), "RE_ Meeting (02)");
        assert_eq!(by_id(1), "RE_ Meeting (03)");
        assert_eq!(plan.counters.collision_groups, 1);
        assert_eq!(plan.counters.largest_collision_group, 3);
        clean(&plan, &policy);
    }

    #[test]
    fn a_hundred_same_subject_messages_widen_the_suffix() {
        let messages: Vec<SourceMessage> = (0..120u64)
            .map(|i| SourceMessage {
                id: i,
                subject: None,
                time: Some(i as i64),
                attachments: vec![],
            })
            .collect();
        let root = folder(0, "root", vec![], messages);
        let policy = Policy::new(20);
        let plan = plan_export(&root, &policy);
        let names: Vec<String> = plan
            .entries
            .iter()
            .map(|e| e.components[0].clone())
            .collect();
        assert_eq!(names.len(), 120);
        assert!(names.contains(&"(no subject)".to_string()));
        assert!(names.contains(&"(no subject) (002)".to_string()));
        assert!(names.contains(&"(no subject) (120)".to_string()));
        assert_eq!(plan.counters.collision_groups_100_plus, 1);
        assert_eq!(plan.counters.names_fallback, 120);
        clean(&plan, &policy);
    }

    #[test]
    fn reserved_and_device_names_are_handled() {
        let root = folder(
            0,
            "root",
            vec![
                folder(1, "folder.json", vec![], vec![]),
                folder(2, "Folder.JSON", vec![], vec![]),
            ],
            vec![msg(10, "NUL", None, vec![]), msg(11, "a:b", None, vec![])],
        );
        let policy = Policy::new(20);
        let plan = plan_export(&root, &policy);
        assert_eq!(
            paths(&plan),
            vec!["Folder.JSON (03)", "NUL_", "a_b", "folder.json (02)"]
        );
        assert_eq!(plan.counters.reserved_name_hits, 1);
        assert_eq!(plan.counters.names_sanitized, 2);
        clean(&plan, &policy);
    }

    #[test]
    fn a_folder_and_a_message_with_the_same_name_do_not_collide_on_disk() {
        let root = folder(
            0,
            "root",
            vec![folder(1, "Budget", vec![], vec![])],
            vec![msg(10, "budget", Some(1), vec![])],
        );
        let policy = Policy::new(20);
        let plan = plan_export(&root, &policy);
        assert_eq!(paths(&plan), vec!["Budget", "budget (02)"]);
        clean(&plan, &policy);
    }

    #[test]
    fn long_subjects_are_shortened_to_fit_a_long_root() {
        let long = "x".repeat(200);
        let root = folder(0, "root", vec![], vec![msg(1, &long, None, vec![])]);
        let policy = Policy::new(150);
        let plan = plan_export(&root, &policy);
        let e = &plan.entries[0];
        assert!(e.flags.contains(&NameFlag::Truncated));
        assert!(e.components[0].ends_with('…'));
        // Directory path <= 247 and metadata.json path <= 259.
        assert!(e.path_units <= 247);
        assert!(e.worst_units <= 259);
        clean(&plan, &policy);
        assert_eq!(plan.counters.names_truncated, 1);
    }

    #[test]
    fn long_attachment_names_are_capped_and_keep_their_extension() {
        let name = format!("{}.pdf", "y".repeat(300));
        let root = folder(
            0,
            "root",
            vec![],
            vec![msg(1, "Report", None, vec![file(1, &name), file(2, &name)])],
        );
        let policy = Policy::new(60);
        let plan = plan_export(&root, &policy);
        let files: Vec<&PlanEntry> = plan
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::AttachmentFile)
            .collect();
        assert_eq!(files.len(), 2);
        for f in &files {
            let last = f.components.last().unwrap();
            assert!(utf16_len(last) <= 100, "{last}");
            assert!(last.ends_with(".pdf"), "{last}");
        }
        assert_ne!(files[0].components.last(), files[1].components.last());
        clean(&plan, &policy);
    }

    #[test]
    fn duplicate_and_missing_attachment_names_are_numbered() {
        let root = folder(
            0,
            "root",
            vec![],
            vec![msg(
                1,
                "M",
                None,
                vec![
                    file(1, "image001.png"),
                    file(2, "image001.png"),
                    SourceAttachment {
                        id: 3,
                        name: None,
                        embedded: None,
                    },
                    file(4, "desktop.ini"),
                ],
            )],
        );
        let policy = Policy::new(30);
        let plan = plan_export(&root, &policy);
        assert_eq!(
            paths(&plan),
            vec![
                "M",
                "M/attachments/_desktop.ini",
                "M/attachments/attachment-3",
                "M/attachments/image001 (02).png",
                "M/attachments/image001.png",
            ]
        );
        clean(&plan, &policy);
    }

    fn nested(levels: u64) -> SourceMessage {
        if levels == 0 {
            msg(100, "leaf", None, vec![file(1, "f.txt")])
        } else {
            msg(
                levels,
                &format!("level {levels}"),
                None,
                vec![embedded(1, nested(levels - 1))],
            )
        }
    }

    #[test]
    fn embedded_messages_nest_to_the_depth_cap_and_are_then_flagged() {
        let root = folder(0, "root", vec![], vec![nested(5)]);
        let policy = Policy::new(30);
        let plan = plan_export(&root, &policy);
        assert_eq!(plan.counters.embedded_messages, 4);
        assert_eq!(plan.counters.embedded_depth_capped, 1);
        let capped: Vec<&PlanEntry> = plan
            .entries
            .iter()
            .filter(|e| e.flags.contains(&NameFlag::DepthCapped))
            .collect();
        assert_eq!(capped.len(), 1);
        // The capped message is written as a directory but nothing is planned below it.
        let cap_path = &capped[0].components;
        assert!(plan
            .entries
            .iter()
            .all(|e| !(e.components.len() > cap_path.len() && e.components.starts_with(cap_path))));
        clean(&plan, &policy);
    }

    #[test]
    fn the_plan_does_not_depend_on_the_order_of_the_source_tree() {
        fn reversed(f: &SourceFolder) -> SourceFolder {
            SourceFolder {
                id: f.id,
                name: f.name.clone(),
                folders: f.folders.iter().rev().map(reversed).collect(),
                messages: f.messages.iter().rev().cloned().collect(),
            }
        }
        let root = folder(
            0,
            "root",
            vec![
                folder(
                    1,
                    "A",
                    vec![],
                    vec![msg(5, "x", Some(2), vec![]), msg(6, "x", Some(1), vec![])],
                ),
                folder(2, "a", vec![], vec![msg(7, "y", None, vec![])]),
            ],
            vec![
                msg(8, "x", Some(3), vec![file(1, "f.txt")]),
                msg(9, "X", Some(3), vec![]),
            ],
        );
        let policy = Policy::new(25);
        let a = plan_export(&root, &policy);
        let b = plan_export(&reversed(&root), &policy);
        let key = |p: &Plan| -> Vec<(EntryKind, u64, Vec<String>)> {
            p.entries
                .iter()
                .map(|e| (e.kind, e.source_id, e.components.clone()))
                .collect()
        };
        assert_eq!(key(&a), key(&b));
        assert_eq!(a.counters, b.counters);
        clean(&a, &policy);
    }

    #[test]
    fn an_impossible_budget_is_reported_not_hidden() {
        // A root so long that even the shortest names cannot fit.
        let root = folder(0, "root", vec![], vec![msg(1, "Subject", None, vec![])]);
        let policy = Policy::new(250);
        let plan = plan_export(&root, &policy);
        assert!(plan.counters.budget_exceeded >= 1);
        assert!(verify_plan(&plan, &policy).over_budget >= 1);
    }

    #[test]
    fn verify_plan_catches_a_collision_and_a_bad_name() {
        let policy = Policy::new(10);
        let entry = |name: &str| PlanEntry {
            kind: EntryKind::Message,
            source_id: 1,
            components: vec![name.to_string()],
            path_units: 10 + 1 + utf16_len(name),
            worst_units: 10 + 1 + utf16_len(name) + 14,
            flags: BTreeSet::new(),
            original: String::new(),
        };
        let plan = Plan {
            entries: vec![
                entry("Budget"),
                entry("budget"),
                entry("NUL"),
                entry("folder.json"),
            ],
            counters: PlanCounters::default(),
        };
        let r = verify_plan(&plan, &policy);
        assert_eq!(r.collisions, 2); // budget/Budget, and folder.json against the reserved name
        assert_eq!(r.invalid_names, 1); // NUL
        assert_eq!(r.over_budget, 0);
        assert_eq!(r.unit_mismatches, 0);
    }

    type MsgSpec = (Option<String>, Option<i64>, Vec<Option<String>>);
    type FolderSpec = (String, Vec<MsgSpec>, Vec<(String, Vec<MsgSpec>)>);

    fn name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            "[ -~]{0,30}",
            prop::sample::select(vec![
                "NUL".to_string(),
                "con.txt".to_string(),
                "a/b:c".to_string(),
                "folder.json".to_string(),
                "...".to_string(),
                "  ".to_string(),
                "e\u{0301}".to_string(),
                "\u{00E9}".to_string(),
                "😀".repeat(40),
                "z".repeat(200),
                "report.pdf".to_string(),
            ]),
        ]
    }

    fn msg_strategy() -> impl Strategy<Value = MsgSpec> {
        (
            prop::option::of(name_strategy()),
            prop::option::of(0i64..4),
            prop::collection::vec(prop::option::of(name_strategy()), 0..4),
        )
    }

    fn tree_strategy() -> impl Strategy<Value = Vec<FolderSpec>> {
        prop::collection::vec(
            (
                name_strategy(),
                prop::collection::vec(msg_strategy(), 0..5),
                prop::collection::vec(
                    (name_strategy(), prop::collection::vec(msg_strategy(), 0..4)),
                    0..3,
                ),
            ),
            0..4,
        )
    }

    fn bump(next: &mut u64) -> u64 {
        *next += 1;
        *next
    }

    fn build_msg(spec: &MsgSpec, next: &mut u64) -> SourceMessage {
        let id = bump(next);
        let attachments = spec
            .2
            .iter()
            .map(|n| SourceAttachment {
                id: bump(next),
                name: n.clone(),
                embedded: None,
            })
            .collect();
        SourceMessage {
            id,
            subject: spec.0.clone(),
            time: spec.1,
            attachments,
        }
    }

    fn build(specs: &[FolderSpec]) -> SourceFolder {
        let mut next = 1u64;
        let mut folders = Vec::new();
        for (name, msgs, subs) in specs {
            let messages: Vec<SourceMessage> =
                msgs.iter().map(|m| build_msg(m, &mut next)).collect();
            let mut subfolders = Vec::new();
            for (sname, smsgs) in subs {
                let sub_messages: Vec<SourceMessage> =
                    smsgs.iter().map(|m| build_msg(m, &mut next)).collect();
                subfolders.push(SourceFolder {
                    id: bump(&mut next),
                    name: sname.clone(),
                    folders: vec![],
                    messages: sub_messages,
                });
            }
            folders.push(SourceFolder {
                id: bump(&mut next),
                name: name.clone(),
                folders: subfolders,
                messages,
            });
        }
        SourceFolder {
            id: 0,
            name: "root".to_string(),
            folders,
            messages: vec![],
        }
    }

    proptest! {
        #[test]
        fn plans_of_arbitrary_trees_pass_every_gate(specs in tree_strategy(), root in 20usize..100) {
            let tree = build(&specs);
            let policy = Policy::new(root);
            let plan = plan_export(&tree, &policy);
            let report = verify_plan(&plan, &policy);
            prop_assert_eq!(&report, &GateReport::default(), "{:?}", paths(&plan));
            prop_assert_eq!(plan.counters.budget_exceeded, 0);
            // Every source item is planned exactly once.
            let expected_folders: usize = specs.iter().map(|f| 1 + f.2.len()).sum();
            prop_assert_eq!(plan.counters.folders, expected_folders);
        }

        #[test]
        fn plans_ignore_the_order_of_folders_and_messages(specs in tree_strategy(), root in 20usize..100) {
            let tree = build(&specs);
            let mut flipped = tree.clone();
            flipped.folders.reverse();
            for f in &mut flipped.folders {
                f.folders.reverse();
                f.messages.reverse();
                for s in &mut f.folders {
                    s.messages.reverse();
                }
            }
            let policy = Policy::new(root);
            let a = plan_export(&tree, &policy);
            let b = plan_export(&flipped, &policy);
            let key = |p: &Plan| -> Vec<(EntryKind, u64, Vec<String>)> {
                p.entries.iter().map(|e| (e.kind, e.source_id, e.components.clone())).collect()
            };
            prop_assert_eq!(key(&a), key(&b));
        }
    }
}
