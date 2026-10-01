//! Custom MS-OXMSG parser: container naming conventions and entry classification (the structural
//! layer).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::shared::CompoundFile;

// =============================================================================
// Custom MS-OXMSG parser: container naming conventions and entry
// classification (the custom parser's structural layer)
// =============================================================================
//
// MS-OXMSG stores every message property inside a CFB (MS-CFB) container as
// one of two things:
// - fixed-length properties, packed together inside a single stream named
//   `__properties_version1.0`;
// - variable-length properties (strings, binary, and variable-length
//   multi-valued values) in streams named `__substg1.0_PPPPTTTT`, where PPPP
//   is the 4-hex-digit property ID and TTTT the 4-hex-digit property type.
//   Variable-length multi-valued values add a zero-based `-NNNNNNNN`
//   value-index suffix.
// Recipients and attachments each get their own numbered sub-storage
// (`__recip_version1.0_#NNNNNNNN`, `__attach_version1.0_#NNNNNNNN`).
// Embedded/custom-object storage is represented by
// `__substg1.0_3701000D`, while named (non-standard) properties get a
// dedicated `__nameid_version1.0` storage. All reporting here is
// name/size/count only -- never content.

/// The CFB storage representing an embedded object (a nested message or a
/// custom/OLE attachment payload), per MS-OXMSG.
pub(crate) const EMBEDDED_OBJECT_STORAGE_NAME: &str = "__substg1.0_3701000D";

#[derive(Clone, Copy)]
pub(crate) enum OxmsgEntryKind {
    Root,
    PropertiesStream,
    PropertyStream { prop_id: u16, indexed: bool },
    AttachmentStorage,
    RecipientStorage,
    NamedPropertyStorage,
    EmbeddedObjectStorage,
    Unrecognized,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OxmsgEntryScope {
    Message,
    Recipient,
    Attachment,
    EmbeddedObject,
    NamedPropertyStorage,
}

impl OxmsgEntryScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Recipient => "recipient",
            Self::Attachment => "attachment",
            Self::EmbeddedObject => "embedded_object",
            Self::NamedPropertyStorage => "named_property_storage",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum CfbObjectKind {
    Storage,
    Stream,
}

impl CfbObjectKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Stream => "stream",
        }
    }
}

pub(crate) fn cfb_object_kind(is_stream: bool) -> CfbObjectKind {
    if is_stream {
        CfbObjectKind::Stream
    } else {
        CfbObjectKind::Storage
    }
}

pub(crate) fn cfb_entry_depth(path: &Path) -> u64 {
    path.components().count().saturating_sub(1) as u64
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum UnrecognizedNameShape {
    MalformedPropertyStream,
    OtherReserved,
    Other,
}

impl UnrecognizedNameShape {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MalformedPropertyStream => "malformed_property_stream_name",
            Self::OtherReserved => "other_reserved_name",
            Self::Other => "other_name",
        }
    }
}

pub(crate) fn unrecognized_name_shape(name: &str) -> UnrecognizedNameShape {
    if name.starts_with("__substg1.0_") {
        UnrecognizedNameShape::MalformedPropertyStream
    } else if name.starts_with("__") {
        UnrecognizedNameShape::OtherReserved
    } else {
        UnrecognizedNameShape::Other
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RecognizedNameTypeMismatch {
    StorageNameIsStream,
    StreamNameIsStorage,
}

impl RecognizedNameTypeMismatch {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StorageNameIsStream => "storage_name_is_stream",
            Self::StreamNameIsStorage => "stream_name_is_storage",
        }
    }
}

pub(crate) fn recognized_name_type_mismatch(
    entry_kind: &OxmsgEntryKind,
    object_kind: CfbObjectKind,
) -> Option<RecognizedNameTypeMismatch> {
    match (entry_kind, object_kind) {
        (
            OxmsgEntryKind::PropertiesStream | OxmsgEntryKind::PropertyStream { .. },
            CfbObjectKind::Storage,
        ) => Some(RecognizedNameTypeMismatch::StreamNameIsStorage),
        (
            OxmsgEntryKind::AttachmentStorage
            | OxmsgEntryKind::RecipientStorage
            | OxmsgEntryKind::NamedPropertyStorage
            | OxmsgEntryKind::EmbeddedObjectStorage,
            CfbObjectKind::Stream,
        ) => Some(RecognizedNameTypeMismatch::StorageNameIsStream),
        _ => None,
    }
}

/// Classifies a single CFB entry by name alone, using the MS-OXMSG
/// storage/stream naming conventions (see the section comment above).
/// Takes a plain `&str` rather than a `cfb::Entry` directly -- that type
/// has no public constructor, so keeping the classification logic pure and
/// string-based is what makes it unit-testable without a real CFB file on
/// disk.
pub(crate) fn classify_oxmsg_entry(name: &str, is_root: bool) -> OxmsgEntryKind {
    if is_root {
        return OxmsgEntryKind::Root;
    }
    if name == "__properties_version1.0" {
        return OxmsgEntryKind::PropertiesStream;
    }
    if name == "__nameid_version1.0" {
        return OxmsgEntryKind::NamedPropertyStorage;
    }
    if name.starts_with("__attach_version1.0_#") {
        return OxmsgEntryKind::AttachmentStorage;
    }
    if name.starts_with("__recip_version1.0_#") {
        return OxmsgEntryKind::RecipientStorage;
    }
    if name == EMBEDDED_OBJECT_STORAGE_NAME {
        return OxmsgEntryKind::EmbeddedObjectStorage;
    }
    if let Some((prop_id, indexed)) = parse_property_stream_name(name) {
        return OxmsgEntryKind::PropertyStream { prop_id, indexed };
    }
    OxmsgEntryKind::Unrecognized
}

pub(crate) fn parse_property_stream_name(name: &str) -> Option<(u16, bool)> {
    let suffix = name.strip_prefix("__substg1.0_")?;

    if suffix.len() == 8 {
        let prop_id = u16::from_str_radix(&suffix[0..4], 16).ok()?;
        u16::from_str_radix(&suffix[4..8], 16).ok()?;
        return Some((prop_id, false));
    }

    let (tag, index) = suffix.split_once('-')?;
    if tag.len() != 8 || index.len() != 8 {
        return None;
    }

    let prop_id = u16::from_str_radix(&tag[0..4], 16).ok()?;
    u16::from_str_radix(&tag[4..8], 16).ok()?;
    u32::from_str_radix(index, 16).ok()?;

    Some((prop_id, true))
}

pub(crate) fn oxmsg_entry_scope(path: &Path) -> OxmsgEntryScope {
    let mut scope = OxmsgEntryScope::Message;

    for component in path.components() {
        let Some(name) = component.as_os_str().to_str() else {
            continue;
        };

        match name {
            "__nameid_version1.0" => {
                scope = OxmsgEntryScope::NamedPropertyStorage;
            }
            EMBEDDED_OBJECT_STORAGE_NAME => {
                scope = OxmsgEntryScope::EmbeddedObject;
            }
            name if name.starts_with("__attach_version1.0_#") => {
                scope = OxmsgEntryScope::Attachment;
            }
            name if name.starts_with("__recip_version1.0_#") => {
                scope = OxmsgEntryScope::Recipient;
            }
            _ => {}
        }
    }

    scope
}

/// Parents of every `__properties_version1.0` stream. A `3701000D` storage
/// in this set is message-shaped (an embedded message); one not in it is a
/// custom attachment storage.
pub(crate) fn message_shaped_parent_paths(comp: &CompoundFile) -> BTreeSet<PathBuf> {
    comp.walk()
        .filter(|e| e.is_stream() && e.name() == "__properties_version1.0")
        .filter_map(|e| e.path().parent().map(Path::to_path_buf))
        .collect()
}

/// If `path` lies beneath a custom (non-message-shaped) embedded-object
/// storage, returns the outermost such storage's path. The storage itself
/// is not "beneath" itself, so it keeps its own classification.
pub(crate) fn enclosing_custom_payload_root(
    path: &Path,
    message_shaped: &BTreeSet<PathBuf>,
) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .filter(|a| a.file_name().and_then(|n| n.to_str()) == Some(EMBEDDED_OBJECT_STORAGE_NAME))
        .filter(|a| !message_shaped.contains(*a))
        .last()
        .map(Path::to_path_buf)
}

/// Privacy-safe ancestry: fixed-vocabulary tokens only, never entry names.
pub(crate) fn oxmsg_ancestry_shape(path: &Path) -> String {
    let mut tokens: Vec<&'static str> = Vec::new();
    let ancestors: Vec<&Path> = path.ancestors().skip(1).collect();
    for ancestor in ancestors.iter().rev() {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue; // the root has no file name
        };
        tokens.push(match name {
            "__nameid_version1.0" => "named_property_storage",
            EMBEDDED_OBJECT_STORAGE_NAME => "embedded_object",
            n if n.starts_with("__attach_version1.0_#") => "attachment",
            n if n.starts_with("__recip_version1.0_#") => "recipient",
            _ => "other_storage",
        });
    }
    if tokens.is_empty() {
        "root".to_string()
    } else {
        format!("root/{}", tokens.join("/"))
    }
}

pub(crate) fn cfb_entry_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

/// Paths of every TOP-LEVEL storage of the given kind. `comp.walk()`
/// traverses the whole tree, including storages nested inside an embedded
/// message's own subtree -- those share the `__recip_version1.0_#*` /
/// `__attach_version1.0_#*` name shape but belong to the inner message.
/// `msg_parser` never opens embedded messages, so its `to`/`cc`/`bcc`/
/// `attachments` only ever reflect the outer message; this filter keeps the
/// custom and msg_parser paths comparing the same scope.
pub(crate) fn top_level_storage_paths(comp: &CompoundFile, kind: OxmsgEntryKind) -> Vec<PathBuf> {
    comp.walk()
        .filter(|e| {
            e.path().parent() == Some(Path::new("/"))
                && matches!(
                    (
                        classify_oxmsg_entry(&cfb_entry_name(e.path()), e.is_root()),
                        kind,
                    ),
                    (
                        OxmsgEntryKind::RecipientStorage,
                        OxmsgEntryKind::RecipientStorage
                    ) | (
                        OxmsgEntryKind::AttachmentStorage,
                        OxmsgEntryKind::AttachmentStorage
                    )
                )
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}
