//! Custom MS-OXMSG structural accounting: aggregate counters, the two-pass walk, and the
//! breakdown report (gated and printed by `--verify`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::oxmsg_classify::{
    cfb_entry_depth, cfb_entry_name, cfb_object_kind, classify_oxmsg_entry,
    enclosing_custom_payload_root, message_shaped_parent_paths, oxmsg_ancestry_shape,
    oxmsg_entry_scope, recognized_name_type_mismatch, unrecognized_name_shape, CfbObjectKind,
    OxmsgEntryKind, OxmsgEntryScope, RecognizedNameTypeMismatch, UnrecognizedNameShape,
};
use crate::oxmsg_decode::{
    classify_property_entry_shape, decode_fixed_value, decode_named_property_string,
    decode_properties_stream, decode_string8_with_codepage, decode_unicode_value,
    expected_size_field_value, expected_variable_stream_path, extract_string8_codepage,
    is_valid_boolean_encoding, properties_stream_header_len, read_named_property_map,
    read_stream_bytes, CodepageSource, DecodedFixedValue, DecodedPropertyEntry, NamedPropertyMap,
    NamedPropertySet, PropertyEntryShape, String8Decoded,
};
use crate::shared::CompoundFile;

// =============================================================================
// Custom MS-OXMSG structural accounting (gated and reported by --verify):
// aggregate counters, breakdown report, and two-pass walk
//
// Pass 1 classifies every CFB entry by name and position (immutable walk);
// pass 2 decodes the entry array of every properties stream found. The
// original single ~320-line `inspect_oxmsg` held three jobs at once; it is
// split so each function does one thing (SLAP). Behavior and counters are
// unchanged.
// =============================================================================

#[derive(Default)]
pub(crate) struct OxmsgTotals {
    pub(crate) total_entries: u64,
    pub(crate) root_entries_total: u64,
    pub(crate) recognized_entries_total: u64,

    pub(crate) has_properties_stream: u64,
    /// Every `__properties_version1.0` entry, including property streams
    /// inside recipient, attachment, and embedded-message storages.
    pub(crate) properties_stream_entries_total: u64,
    pub(crate) properties_stream_bytes_total: u64,

    pub(crate) property_streams_total: u64,
    /// Property streams whose names include the zero-based value index
    /// used by variable-length multiple-valued properties.
    pub(crate) indexed_property_streams_total: u64,
    /// Aggregate counts by MAPI property ID across every file scanned.
    /// Property IDs are a bounded, standard MAPI vocabulary, not user
    /// content. Excludes named-property-storage streams.
    pub(crate) property_id_counts: BTreeMap<u16, u64>,
    /// `__properties_version1.0` stream counts separated by the containing
    /// MS-OXMSG object scope. The named-property mapping storage is
    /// intentionally distinct from ordinary message/recipient/attachment
    /// property scopes.
    pub(crate) properties_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,
    /// `__substg1.0_*` property-stream counts separated by scope.
    pub(crate) property_streams_by_scope: BTreeMap<OxmsgEntryScope, u64>,
    /// Property ID counts separated by the containing MS-OXMSG object
    /// scope.
    pub(crate) property_id_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,

    pub(crate) attachment_storages_total: u64,
    pub(crate) recipient_storages_total: u64,
    pub(crate) named_property_storages_total: u64,
    pub(crate) embedded_object_storages_total: u64,

    /// Entries whose name matched none of the known MS-OXMSG conventions.
    /// Counted, never silently dropped: a nonzero count means either an
    /// MS-OXMSG structure this parser doesn't know about yet, or a real
    /// anomaly worth a closer look.
    pub(crate) unrecognized_entries_total: u64,
    /// Privacy-safe structural breakdown. Names and paths are never
    /// emitted.
    pub(crate) unrecognized_entries:
        BTreeMap<(CfbObjectKind, u64, UnrecognizedNameShape, String), u64>,
    /// A recognized MS-OXMSG name whose CFB object type is unexpected.
    pub(crate) recognized_name_type_mismatches: BTreeMap<RecognizedNameTypeMismatch, u64>,

    /// Entries beneath a custom (non-message-shaped) embedded-object
    /// storage. Their names are defined by the producing application, not
    /// MS-OXMSG ("Custom Attachment Storage"), so they are counted as
    /// opaque payload rather than matched against MS-OXMSG names.
    pub(crate) opaque_payload_entries_total: u64,
    /// (object kind, depth below the payload root) -> count.
    pub(crate) opaque_payload_entries: BTreeMap<(CfbObjectKind, u64), u64>,
    pub(crate) embedded_object_storages_message_shaped_total: u64,
    pub(crate) embedded_object_storages_custom_total: u64,
    /// (shape, storage CLSID) -> count. CLSIDs are a bounded
    /// class-identifier vocabulary, not user content.
    pub(crate) embedded_object_storages_by_shape: BTreeMap<(&'static str, String), u64>,

    // --- Property-type/value decoding --------------------------------------
    /// Every fixed-length entry decoded from a `__properties_version1.0`
    /// stream's entry array. Entry values are never read or reported --
    /// only structural fields (type, ID, flags, and, for variable-length
    /// entries, size/reserved).
    pub(crate) properties_entries_total: u64,
    pub(crate) properties_entries_fixed_inline_total: u64,
    pub(crate) properties_entries_variable_single_total: u64,
    pub(crate) properties_entries_variable_multivalued_total: u64,
    /// A stream shorter than the header size expected for its scope.
    pub(crate) properties_stream_too_short_for_header_total: u64,
    /// A stream whose length past the header isn't an exact multiple of 16.
    pub(crate) properties_stream_trailing_bytes_total: u64,
    /// A properties stream in a scope with no defined header size (per
    /// MS-OXMSG this should never be Named Property Mapping storage).
    pub(crate) properties_stream_unexpected_scope_total: u64,
    pub(crate) properties_stream_read_errors: u64,
    /// (scope, raw property type incl. the 0x1000 multi-value bit) ->
    /// count. Property types are a bounded MAPI vocabulary
    /// (MS-OXCDATA 2.11.1), not user content.
    pub(crate) property_type_counts_by_scope: BTreeMap<(OxmsgEntryScope, u16), u64>,
    /// Property Entry flags are a 3-bit MS-OXMSG vocabulary (mandatory /
    /// readable / writable), not user content.
    pub(crate) property_entry_flags_counts: BTreeMap<u32, u64>,
    /// Reserved-field values seen on the attachment-scope
    /// PidTagAttachDataObject (0x3701, PT_OBJECT) entry. Per MS-OXMSG
    /// 2.4.2.2, this is 0x01 for an embedded-message attachment and 0x04
    /// for a storage (OLE/custom) attachment -- an independent,
    /// property-level cross-check of the CFB-structural
    /// message-shaped/custom classification.
    pub(crate) attach_data_object_reserved_counts: BTreeMap<u32, u64>,
    /// Per spec this entry's Size field MUST be 0xFFFFFFFF; count any
    /// that aren't, rather than assuming.
    pub(crate) attach_data_object_size_sentinel_mismatches: u64,

    // --- Fixed-value and variable-value structural checks ------------------
    pub(crate) fixed_boolean_invalid_encoding_total: u64,
    /// A decoded PT_FLOAT/PT_DOUBLE/PT_APPTIME value that is NaN or
    /// infinite. Not necessarily invalid data on its own, but implausible
    /// for the values these types are normally used for -- worth
    /// investigating as a possible decode-path bug before assuming it's
    /// genuine.
    pub(crate) fixed_float_non_finite_total: u64,
    pub(crate) variable_value_stream_found_total: u64,
    pub(crate) variable_value_stream_missing_total: u64,
    pub(crate) variable_value_size_mismatch_total: u64,
    pub(crate) variable_value_odd_utf16_length_total: u64,
    /// PT_UNICODE bytes (already confirmed even-length) that still fail to
    /// decode as valid UTF-16 -- e.g. an unpaired surrogate.
    pub(crate) variable_unicode_decode_errors_total: u64,
    /// Bytes replaced with U+FFFD while decoding a PT_STRING8 value as
    /// Windows-1252 -- unverified against real data.
    pub(crate) variable_string8_undefined_byte_total: u64,
    /// PT_STRING8 value streams under a code page this decoder does not
    /// implement, with non-ASCII bytes (no text produced).
    pub(crate) variable_string8_unsupported_codepage_total: u64,
    /// PT_STRING8 value streams that are all ASCII under a known ASCII
    /// superset code page this decoder does not otherwise implement.
    pub(crate) variable_string8_ascii_under_unsupported_codepage_total: u64,
    /// Which link of the code page chain resolved, once per file.
    pub(crate) string8_codepage_from_message_total: u64,
    pub(crate) string8_codepage_from_internet_total: u64,
    pub(crate) string8_codepage_fallback_total: u64,
    /// A PT_CLSID (0x0048) value stream whose length isn't exactly the 16
    /// bytes a GUID requires.
    pub(crate) variable_clsid_wrong_length_total: u64,
    // --- Named-property resolution -----------------------------------------
    pub(crate) named_properties_seen_total: u64,
    pub(crate) named_properties_map_missing_total: u64,
    pub(crate) named_properties_unresolvable_total: u64,
    pub(crate) named_properties_guid_out_of_range_total: u64,
    pub(crate) named_properties_string_kind_total: u64,
    pub(crate) named_properties_numeric_kind_total: u64,
    /// Property-set membership, by bounded label ("PS_MAPI",
    /// "PS_PUBLIC_STRINGS", a well-known PSETID name, or "custom").
    pub(crate) named_property_sets: BTreeMap<&'static str, u64>,
    /// Numeric LIDs are small application-defined integers, not content --
    /// same footing as a property ID.
    pub(crate) named_property_numeric_lids: BTreeMap<u32, u64>,
    /// Whether a string-kind named property's name decoded successfully --
    /// never the name itself.
    pub(crate) named_properties_string_decode_errors_total: u64,
    /// A resolved entry whose own claimed Property Index doesn't match the
    /// array position it was looked up by. Per MS-OXMSG this MUST always
    /// match; a permanent cross-check now that the bit layout is
    /// confirmed.
    pub(crate) named_properties_index_mismatch_total: u64,
}

impl OxmsgTotals {
    /// Difference between every enumerated CFB entry and the classified
    /// categories. This remains signed so a future overcount is visible.
    pub(crate) fn entry_accounting_gap_total(&self) -> i64 {
        self.total_entries as i64
            - self.recognized_entries_total as i64
            - self.opaque_payload_entries_total as i64
            - self.unrecognized_entries_total as i64
    }
}

/// Everything `inspect_oxmsg` needs from a CFB entry, captured up front so
/// the immutable borrow from `comp.walk()` ends before the second pass
/// needs `&mut comp` to read stream contents.
pub(crate) struct CollectedOxmsgEntry {
    pub(crate) path: PathBuf,
    pub(crate) is_root: bool,
    pub(crate) is_stream: bool,
    pub(crate) len: u64,
    pub(crate) clsid: String,
}

/// Prints the full structural breakdown (entry accounting, per-scope and
/// per-type property counts, the privacy-safe shape of anything
/// unrecognized), separated from the scan loop (SLAP). `--verify` prints it
/// only when a structural gate is nonzero, to make the violation
/// triageable; keys keep the vocabulary the retired `--oxmsg` report used,
/// so some repeat the gate lines with identical values.
pub(crate) fn print_structural_breakdown(totals: &OxmsgTotals) {
    println!("total_entries={}", totals.total_entries);
    println!("root_entries_total={}", totals.root_entries_total);
    println!(
        "recognized_entries_total={}",
        totals.recognized_entries_total
    );
    println!(
        "entry_accounting_gap_total={}",
        totals.entry_accounting_gap_total()
    );
    println!("has_properties_stream={}", totals.has_properties_stream);
    println!(
        "properties_stream_entries_total={}",
        totals.properties_stream_entries_total
    );
    println!(
        "properties_stream_bytes_total={}",
        totals.properties_stream_bytes_total
    );
    println!("property_streams_total={}", totals.property_streams_total);
    println!(
        "indexed_property_streams_total={}",
        totals.indexed_property_streams_total
    );
    println!(
        "embedded_object_storages_total={}",
        totals.embedded_object_storages_total
    );
    println!(
        "attachment_storages_total={}",
        totals.attachment_storages_total
    );
    println!(
        "recipient_storages_total={}",
        totals.recipient_storages_total
    );
    println!(
        "named_property_storages_total={}",
        totals.named_property_storages_total
    );
    // Keep the original presence field for consumers that only need a
    // compatibility-friendly yes/no result.
    println!(
        "has_named_property_storage={}",
        totals.named_property_storages_total > 0
    );
    println!(
        "unrecognized_entries_total={}",
        totals.unrecognized_entries_total
    );
    println!(
        "embedded_object_storages_message_shaped_total={}",
        totals.embedded_object_storages_message_shaped_total
    );
    println!(
        "embedded_object_storages_custom_total={}",
        totals.embedded_object_storages_custom_total
    );
    for ((shape, clsid), count) in &totals.embedded_object_storages_by_shape {
        println!("embedded_object_storage shape={shape} clsid={clsid} count={count}");
    }
    println!(
        "opaque_payload_entries_total={}",
        totals.opaque_payload_entries_total
    );
    for ((object_kind, below), count) in &totals.opaque_payload_entries {
        println!(
            "opaque_payload_entry kind={} depth_below_payload={below} count={count}",
            object_kind.as_str()
        );
    }
    for ((object_kind, depth, name_shape, ancestry), count) in &totals.unrecognized_entries {
        println!(
            "unrecognized_entry kind={} depth={} name_shape={} ancestry={ancestry} count={count}",
            object_kind.as_str(),
            depth,
            name_shape.as_str()
        );
    }
    for (mismatch, count) in &totals.recognized_name_type_mismatches {
        println!(
            "recognized_name_type_mismatch kind={} count={count}",
            mismatch.as_str()
        );
    }
    for (scope, count) in &totals.properties_streams_by_scope {
        println!("properties_stream scope={} count={count}", scope.as_str());
    }
    for (scope, count) in &totals.property_streams_by_scope {
        println!("property_stream scope={} count={count}", scope.as_str());
    }
    for ((scope, prop_id), count) in &totals.property_id_counts_by_scope {
        println!(
            "property_id scope={} id=0x{prop_id:04X} count={count}",
            scope.as_str()
        );
    }
    for (prop_id, count) in &totals.property_id_counts {
        println!("property_id id=0x{prop_id:04X} count={count}");
    }
    println!(
        "properties_entries_total={}",
        totals.properties_entries_total
    );
    println!(
        "properties_entries_fixed_inline_total={}",
        totals.properties_entries_fixed_inline_total
    );
    println!(
        "properties_entries_variable_single_total={}",
        totals.properties_entries_variable_single_total
    );
    println!(
        "properties_entries_variable_multivalued_total={}",
        totals.properties_entries_variable_multivalued_total
    );
    println!(
        "properties_stream_too_short_for_header_total={}",
        totals.properties_stream_too_short_for_header_total
    );
    println!(
        "properties_stream_trailing_bytes_total={}",
        totals.properties_stream_trailing_bytes_total
    );
    println!(
        "properties_stream_unexpected_scope_total={}",
        totals.properties_stream_unexpected_scope_total
    );
    println!(
        "properties_stream_read_errors={}",
        totals.properties_stream_read_errors
    );
    for ((scope, property_type), count) in &totals.property_type_counts_by_scope {
        println!(
            "property_type scope={} type=0x{property_type:04X} count={count}",
            scope.as_str()
        );
    }
    for (flags, count) in &totals.property_entry_flags_counts {
        println!("property_entry_flags value=0x{flags:X} count={count}");
    }
    for (reserved, count) in &totals.attach_data_object_reserved_counts {
        println!("attach_data_object_entry reserved=0x{reserved:02X} count={count}");
    }
    println!(
        "attach_data_object_size_sentinel_mismatches={}",
        totals.attach_data_object_size_sentinel_mismatches
    );
    let reserved_embedded = totals
        .attach_data_object_reserved_counts
        .get(&0x01)
        .copied()
        .unwrap_or(0);
    let reserved_storage = totals
        .attach_data_object_reserved_counts
        .get(&0x04)
        .copied()
        .unwrap_or(0);
    println!(
        "attach_data_object_reserved_vs_embedded_object_storage_gap={}",
        reserved_embedded as i64 - totals.embedded_object_storages_message_shaped_total as i64
    );
    println!(
        "attach_data_object_reserved_vs_custom_object_storage_gap={}",
        reserved_storage as i64 - totals.embedded_object_storages_custom_total as i64
    );
    println!(
        "fixed_boolean_invalid_encoding_total={}",
        totals.fixed_boolean_invalid_encoding_total
    );
    println!(
        "fixed_float_non_finite_total={}",
        totals.fixed_float_non_finite_total
    );
    println!(
        "variable_value_stream_found_total={}",
        totals.variable_value_stream_found_total
    );
    println!(
        "variable_value_stream_missing_total={}",
        totals.variable_value_stream_missing_total
    );
    println!(
        "variable_value_size_mismatch_total={}",
        totals.variable_value_size_mismatch_total
    );
    println!(
        "variable_value_odd_utf16_length_total={}",
        totals.variable_value_odd_utf16_length_total
    );
    println!(
        "variable_unicode_decode_errors_total={}",
        totals.variable_unicode_decode_errors_total
    );
    println!(
        "variable_string8_undefined_byte_total={}",
        totals.variable_string8_undefined_byte_total
    );
    println!(
        "variable_string8_unsupported_codepage_total={}",
        totals.variable_string8_unsupported_codepage_total
    );
    println!(
        "variable_string8_ascii_under_unsupported_codepage_total={}",
        totals.variable_string8_ascii_under_unsupported_codepage_total
    );
    println!(
        "string8_codepage_from_message_total={}",
        totals.string8_codepage_from_message_total
    );
    println!(
        "string8_codepage_from_internet_total={}",
        totals.string8_codepage_from_internet_total
    );
    println!(
        "string8_codepage_fallback_total={}",
        totals.string8_codepage_fallback_total
    );
    println!(
        "variable_clsid_wrong_length_total={}",
        totals.variable_clsid_wrong_length_total
    );
    println!(
        "named_properties_seen_total={}",
        totals.named_properties_seen_total
    );
    println!(
        "named_properties_map_missing_total={}",
        totals.named_properties_map_missing_total
    );
    println!(
        "named_properties_unresolvable_total={}",
        totals.named_properties_unresolvable_total
    );
    println!(
        "named_properties_guid_out_of_range_total={}",
        totals.named_properties_guid_out_of_range_total
    );
    println!(
        "named_properties_string_kind_total={}",
        totals.named_properties_string_kind_total
    );
    println!(
        "named_properties_numeric_kind_total={}",
        totals.named_properties_numeric_kind_total
    );
    for (set, count) in &totals.named_property_sets {
        println!("named_property_set set={set} count={count}");
    }
    for (lid, count) in &totals.named_property_numeric_lids {
        println!("named_property_numeric_lid lid=0x{lid:04X} count={count}");
    }
    println!(
        "named_properties_string_decode_errors_total={}",
        totals.named_properties_string_decode_errors_total
    );
    println!(
        "named_properties_index_mismatch_total={}",
        totals.named_properties_index_mismatch_total
    );
}

/// Orchestrates the two-pass structural diagnostic over one open .msg
/// container. Reports only structural fields -- type, ID, flags,
/// size/reserved, presence -- never a property's value.
pub(crate) fn inspect_oxmsg(comp: &mut CompoundFile, totals: &mut OxmsgTotals) {
    let message_shaped_parents = message_shaped_parent_paths(&*comp);

    // Pass 1 needs only immutable access; entries are collected up front,
    // ending that borrow before pass 2 needs `&mut comp` to read stream
    // contents.
    let entries: Vec<CollectedOxmsgEntry> = comp
        .walk()
        .map(|e| CollectedOxmsgEntry {
            path: e.path().to_path_buf(),
            is_root: e.is_root(),
            is_stream: e.is_stream(),
            len: e.len(),
            clsid: e.clsid().to_string(),
        })
        .collect();

    let saw_properties_stream = classify_oxmsg_entries(&entries, &message_shaped_parents, totals);
    if saw_properties_stream {
        totals.has_properties_stream += 1;
    }

    // Named properties are resolved once per file (the mapping storage is
    // shared by the whole message, embedded messages included) rather than
    // once per entry.
    let named_property_map = read_named_property_map(comp);

    // The PT_STRING8 code page is resolved once per file from the
    // top-level message's own properties.
    let string8_codepage = extract_string8_codepage(
        comp,
        Path::new("/"),
        properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32),
    );
    match string8_codepage.source {
        CodepageSource::MessageCodepage => totals.string8_codepage_from_message_total += 1,
        CodepageSource::InternetCodepage => totals.string8_codepage_from_internet_total += 1,
        CodepageSource::Fallback => totals.string8_codepage_fallback_total += 1,
    }

    decode_oxmsg_properties_streams(
        comp,
        &entries,
        named_property_map.as_ref(),
        string8_codepage.codepage,
        totals,
    );
}

/// Pass 1: classifies every CFB entry from its name, position, and
/// ancestry, updating the structural counters. Returns whether the file
/// had at least one `__properties_version1.0` stream.
pub(crate) fn classify_oxmsg_entries(
    entries: &[CollectedOxmsgEntry],
    message_shaped_parents: &BTreeSet<PathBuf>,
    totals: &mut OxmsgTotals,
) -> bool {
    let mut saw_properties_stream = false;

    for entry in entries {
        totals.total_entries += 1;
        let object_kind = cfb_object_kind(entry.is_stream);

        // Ancestry outranks name: anything beneath a custom attachment
        // storage is application-defined, whatever it happens to be called.
        if let Some(payload_root) =
            enclosing_custom_payload_root(&entry.path, message_shaped_parents)
        {
            totals.opaque_payload_entries_total += 1;
            let below = cfb_entry_depth(&entry.path).saturating_sub(cfb_entry_depth(&payload_root));
            *totals
                .opaque_payload_entries
                .entry((object_kind, below))
                .or_insert(0) += 1;
            continue;
        }

        let entry_kind = classify_oxmsg_entry(&cfb_entry_name(&entry.path), entry.is_root);
        if let Some(mismatch) = recognized_name_type_mismatch(&entry_kind, object_kind) {
            *totals
                .recognized_name_type_mismatches
                .entry(mismatch)
                .or_insert(0) += 1;
        }
        match entry_kind {
            OxmsgEntryKind::Root => {
                totals.root_entries_total += 1;
                totals.recognized_entries_total += 1;
            }
            OxmsgEntryKind::PropertiesStream => {
                totals.recognized_entries_total += 1;
                totals.properties_stream_entries_total += 1;
                saw_properties_stream = true;
                totals.properties_stream_bytes_total += entry.len;

                let scope = oxmsg_entry_scope(&entry.path);
                *totals.properties_streams_by_scope.entry(scope).or_insert(0) += 1;
            }
            OxmsgEntryKind::PropertyStream { prop_id, indexed } => {
                totals.recognized_entries_total += 1;
                totals.property_streams_total += 1;
                if indexed {
                    totals.indexed_property_streams_total += 1;
                }

                let scope = oxmsg_entry_scope(&entry.path);
                *totals.property_streams_by_scope.entry(scope).or_insert(0) += 1;
                *totals
                    .property_id_counts_by_scope
                    .entry((scope, prop_id))
                    .or_insert(0) += 1;
                // Named-property storage streams are not MAPI properties:
                // their four-hex prefix is a stream identifier (0x0002-0x0004
                // and the 0x1000-range hash buckets), which would collide
                // with real property IDs such as PidTagBody (0x1000).
                if scope != OxmsgEntryScope::NamedPropertyStorage {
                    *totals.property_id_counts.entry(prop_id).or_insert(0) += 1;
                }
            }
            OxmsgEntryKind::AttachmentStorage => {
                totals.recognized_entries_total += 1;
                totals.attachment_storages_total += 1;
            }
            OxmsgEntryKind::RecipientStorage => {
                totals.recognized_entries_total += 1;
                totals.recipient_storages_total += 1;
            }
            OxmsgEntryKind::NamedPropertyStorage => {
                totals.recognized_entries_total += 1;
                totals.named_property_storages_total += 1;
            }
            OxmsgEntryKind::EmbeddedObjectStorage => {
                totals.recognized_entries_total += 1;
                totals.embedded_object_storages_total += 1;
                let shape = if message_shaped_parents.contains(&entry.path) {
                    totals.embedded_object_storages_message_shaped_total += 1;
                    "message_shaped"
                } else {
                    totals.embedded_object_storages_custom_total += 1;
                    "custom"
                };
                *totals
                    .embedded_object_storages_by_shape
                    .entry((shape, entry.clsid.clone()))
                    .or_insert(0) += 1;
            }
            OxmsgEntryKind::Unrecognized => {
                totals.unrecognized_entries_total += 1;
                *totals
                    .unrecognized_entries
                    .entry((
                        object_kind,
                        cfb_entry_depth(&entry.path),
                        unrecognized_name_shape(&cfb_entry_name(&entry.path)),
                        oxmsg_ancestry_shape(&entry.path),
                    ))
                    .or_insert(0) += 1;
            }
        }
    }

    saw_properties_stream
}

/// Pass 2: decodes the entry array of every `__properties_version1.0`
/// stream found in pass 1. This reads stream contents, but reports only
/// structural fields -- never a property's value.
pub(crate) fn decode_oxmsg_properties_streams(
    comp: &mut CompoundFile,
    entries: &[CollectedOxmsgEntry],
    named_property_map: Option<&NamedPropertyMap>,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    for entry in entries {
        if !entry.is_stream || cfb_entry_name(&entry.path) != "__properties_version1.0" {
            continue;
        }
        let scope = oxmsg_entry_scope(&entry.path);
        let Some(header_len) = properties_stream_header_len(scope) else {
            totals.properties_stream_unexpected_scope_total += 1;
            continue;
        };
        let Some(bytes) = read_stream_bytes(comp, &entry.path) else {
            totals.properties_stream_read_errors += 1;
            continue;
        };
        let Some(decoded) = decode_properties_stream(&bytes, header_len) else {
            totals.properties_stream_too_short_for_header_total += 1;
            continue;
        };
        if decoded.trailing_bytes != 0 {
            totals.properties_stream_trailing_bytes_total += 1;
        }
        for prop_entry in &decoded.entries {
            record_property_entry(
                comp,
                &entry.path,
                scope,
                prop_entry,
                named_property_map,
                string8_codepage,
                totals,
            );
        }
    }
}

/// Runs every per-entry structural check for one decoded Property Entry:
/// type/flags accounting, fixed-value validity, the variable-length value
/// stream cross-check, the attach-data-object cross-check, and
/// named-property resolution.
pub(crate) fn record_property_entry(
    comp: &mut CompoundFile,
    stream_path: &Path,
    scope: OxmsgEntryScope,
    entry: &DecodedPropertyEntry,
    named_property_map: Option<&NamedPropertyMap>,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    totals.properties_entries_total += 1;
    *totals
        .property_type_counts_by_scope
        .entry((scope, entry.property_type))
        .or_insert(0) += 1;
    *totals
        .property_entry_flags_counts
        .entry(entry.flags)
        .or_insert(0) += 1;

    match classify_property_entry_shape(entry.property_type) {
        PropertyEntryShape::FixedInline => check_fixed_inline_entry(entry, totals),
        PropertyEntryShape::VariableSingle => {
            totals.properties_entries_variable_single_total += 1;
            check_variable_value_stream(comp, stream_path, entry, string8_codepage, totals);
        }
        PropertyEntryShape::VariableMultivalued => {
            totals.properties_entries_variable_multivalued_total += 1;
        }
    }

    record_attach_data_object_entry(scope, entry, totals);
    record_named_property_observation(entry.property_id, named_property_map, totals);
}

/// Structural checks for one fixed-length entry whose value is stored
/// inline in the entry's 8-byte value field. The value itself is decoded
/// only to check plausibility (non-finite floats) -- never read or
/// reported as content.
pub(crate) fn check_fixed_inline_entry(entry: &DecodedPropertyEntry, totals: &mut OxmsgTotals) {
    totals.properties_entries_fixed_inline_total += 1;
    if entry.property_type == 0x000B && !is_valid_boolean_encoding(&entry.tail) {
        totals.fixed_boolean_invalid_encoding_total += 1;
    }
    if let Some(value) = decode_fixed_value(entry.property_type, &entry.tail) {
        let non_finite = match value {
            DecodedFixedValue::Float(f) => !f.is_finite(),
            DecodedFixedValue::Double(d) | DecodedFixedValue::AppTime(d) => !d.is_finite(),
            _ => false,
        };
        if non_finite {
            totals.fixed_float_non_finite_total += 1;
        }
    }
}

/// Cross-checks one variable-length single-value entry against the
/// `__substg1.0_PPPPTTTT` stream MS-OXMSG 2.4.2.2 says must hold its
/// value: presence, declared-size agreement, and per-type decodability.
/// PT_OBJECT (0x000D) properties point at a storage, not a stream -- they
/// are already covered by the embedded-object accounting in pass 1, so
/// they are skipped here. Never reads a value as content.
pub(crate) fn check_variable_value_stream(
    comp: &mut CompoundFile,
    stream_path: &Path,
    entry: &DecodedPropertyEntry,
    string8_codepage: u32,
    totals: &mut OxmsgTotals,
) {
    let declared_size =
        u32::from_le_bytes([entry.tail[0], entry.tail[1], entry.tail[2], entry.tail[3]]);
    if entry.property_type == 0x000D || declared_size == 0xFFFF_FFFF {
        return;
    }
    let parent = stream_path.parent().unwrap_or(Path::new("/"));
    let value_path = expected_variable_stream_path(parent, entry.property_id, entry.property_type);
    let Some(value_bytes) = read_stream_bytes(comp, &value_path) else {
        totals.variable_value_stream_missing_total += 1;
        return;
    };

    totals.variable_value_stream_found_total += 1;
    let expected = expected_size_field_value(entry.property_type, value_bytes.len() as u64);
    if expected != declared_size as u64 {
        totals.variable_value_size_mismatch_total += 1;
    }
    match entry.property_type {
        0x001F => {
            if value_bytes.len() % 2 != 0 {
                totals.variable_value_odd_utf16_length_total += 1;
            } else if decode_unicode_value(&value_bytes).is_err() {
                totals.variable_unicode_decode_errors_total += 1;
            }
        }
        0x001E => match decode_string8_with_codepage(&value_bytes, string8_codepage) {
            String8Decoded::Decoded { replaced, .. } => {
                totals.variable_string8_undefined_byte_total += replaced as u64;
            }
            String8Decoded::AsciiUnderUnsupportedCodepage { .. } => {
                totals.variable_string8_ascii_under_unsupported_codepage_total += 1;
            }
            String8Decoded::UnsupportedCodepage => {
                totals.variable_string8_unsupported_codepage_total += 1;
            }
        },
        0x0048 if value_bytes.len() != 16 => {
            totals.variable_clsid_wrong_length_total += 1;
        }
        _ => {}
    }
}

/// PidTagAttachDataObject (0x3701, PT_OBJECT 0x000D) on an attachment: an
/// independent, property-level cross-check of the CFB-structural
/// message-shaped/custom classification (MS-OXMSG 2.4.2.2).
pub(crate) fn record_attach_data_object_entry(
    scope: OxmsgEntryScope,
    entry: &DecodedPropertyEntry,
    totals: &mut OxmsgTotals,
) {
    if scope != OxmsgEntryScope::Attachment
        || entry.property_id != 0x3701
        || entry.property_type != 0x000D
    {
        return;
    }
    let size = u32::from_le_bytes([entry.tail[0], entry.tail[1], entry.tail[2], entry.tail[3]]);
    let reserved = u32::from_le_bytes([entry.tail[4], entry.tail[5], entry.tail[6], entry.tail[7]]);
    *totals
        .attach_data_object_reserved_counts
        .entry(reserved)
        .or_insert(0) += 1;
    // Per spec this entry's Size field MUST be 0xFFFFFFFF; count any that
    // aren't, rather than assuming.
    if size != 0xFFFF_FFFF {
        totals.attach_data_object_size_sentinel_mismatches += 1;
    }
}

/// Named-property resolution (MS-OXMSG 2.2.3): identity only (property set
/// + numeric-or-string), never a string name.
pub(crate) fn record_named_property_observation(
    property_id: u16,
    named_property_map: Option<&NamedPropertyMap>,
    totals: &mut OxmsgTotals,
) {
    if property_id < 0x8000 {
        return;
    }
    totals.named_properties_seen_total += 1;
    let Some(map) = named_property_map else {
        totals.named_properties_map_missing_total += 1;
        return;
    };
    let Some(raw) = map.lookup(property_id) else {
        totals.named_properties_unresolvable_total += 1;
        return;
    };

    // Per MS-OXMSG the entry's own claimed Property Index MUST equal its
    // array position; a permanent cross-check now that the bit layout is
    // confirmed.
    let expected_index = property_id - 0x8000;
    if raw.property_index != expected_index {
        totals.named_properties_index_mismatch_total += 1;
    }
    if raw.is_string {
        totals.named_properties_string_kind_total += 1;
        if decode_named_property_string(&map.string_stream, raw.name_id_or_offset).is_none() {
            totals.named_properties_string_decode_errors_total += 1;
        }
    } else {
        totals.named_properties_numeric_kind_total += 1;
        *totals
            .named_property_numeric_lids
            .entry(raw.name_id_or_offset)
            .or_insert(0) += 1;
    }
    match map.resolve_set(raw.guid_index) {
        NamedPropertySet::PsMapi => {
            *totals.named_property_sets.entry("PS_MAPI").or_insert(0) += 1;
        }
        NamedPropertySet::PsPublicStrings => {
            *totals
                .named_property_sets
                .entry("PS_PUBLIC_STRINGS")
                .or_insert(0) += 1;
        }
        NamedPropertySet::WellKnown(name) => {
            *totals.named_property_sets.entry(name).or_insert(0) += 1;
        }
        NamedPropertySet::Custom => {
            *totals.named_property_sets.entry("custom").or_insert(0) += 1;
        }
        NamedPropertySet::OutOfRange => {
            totals.named_properties_guid_out_of_range_total += 1;
        }
    }
}
