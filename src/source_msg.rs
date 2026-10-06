//! Builds the planner's source tree from `.msg` input (M4b-3): subjects, times, attachment names,
//! and embedded messages, read through the custom MS-OXMSG layer. A directory input mirrors its
//! subdirectories as folders. Names are read into memory only; nothing is printed.
//!
//! M4c adds two things for the export writer: the map from each top-level message's source ID to
//! the `.msg` file it came from, and [`read_message_content`], which re-reads one message's
//! content when the writer needs it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::dry_run::{
    strip_subject_marker, InternetIdTracker, SourceCensus, PROP_ATTACH_FILENAME,
    PROP_ATTACH_LONG_FILENAME, PROP_DELIVERY_TIME, PROP_INTERNET_MESSAGE_ID, PROP_SUBJECT,
    PROP_SUBMIT_TIME,
};
use crate::model::{BodyAvailability, MessageContent, PlainBody};
use crate::oxmsg_classify::{
    cfb_entry_name, classify_oxmsg_entry, OxmsgEntryKind, OxmsgEntryScope,
    EMBEDDED_OBJECT_STORAGE_NAME,
};
use crate::oxmsg_decode::{
    decode_fixed_value, decode_properties_stream, extract_string8_codepage,
    properties_stream_header_len, read_stream_bytes, DecodedFixedValue,
};
use crate::oxmsg_envelope::extract_envelope;
use crate::oxmsg_extract::{extract_body_flags, read_string_property};
use crate::plan::{SourceAttachment, SourceFolder, SourceMessage};
use crate::shared::{CompoundFile, PROP_ATTACH_METHOD, PROP_BODY};

/// Deepest directory nesting mirrored (guards against link loops).
const MAX_DIRECTORY_DEPTH: usize = 64;
/// Deepest embedded-message nesting read.
const MAX_EMBEDDED_READ_DEPTH: usize = 8;
const ATTACH_METHOD_EMBEDDED: i32 = 5;

/// Source ID of each top-level message (one that came from a `.msg` file of its own) to that file.
pub(crate) type MsgFileMap = BTreeMap<u64, PathBuf>;

struct Builder {
    next_id: u64,
    census: SourceCensus,
    ids: InternetIdTracker,
    files: MsgFileMap,
}

/// Builds the source tree for a single `.msg` file or a directory of them (recursively), plus the
/// map from each message's source ID to its file.
pub(crate) fn build_msg_export_source(
    input: &Path,
) -> Result<(SourceFolder, SourceCensus, MsgFileMap)> {
    let mut b = Builder {
        next_id: 0,
        census: SourceCensus::default(),
        ids: InternetIdTracker::default(),
        files: MsgFileMap::new(),
    };
    let root = if input.is_dir() {
        b.folder_from_dir(input, 0)?
    } else {
        let mut folder = SourceFolder {
            id: b.bump(),
            name: String::new(),
            folders: Vec::new(),
            messages: Vec::new(),
        };
        if let Some(message) = b.message_from_file(input) {
            folder.messages.push(message);
        }
        folder
    };
    Ok((root, b.census, b.files))
}

/// Builds the source tree for a single `.msg` file or a directory of them (recursively).
pub(crate) fn build_msg_tree(input: &Path) -> Result<(SourceFolder, SourceCensus)> {
    let (root, census, _files) = build_msg_export_source(input)?;
    Ok((root, census))
}

fn is_msg_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("msg"))
}

impl Builder {
    fn bump(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn folder_from_dir(&mut self, dir: &Path, depth: usize) -> Result<SourceFolder> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .context("failed to read input directory")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();

        let mut folder = SourceFolder {
            id: self.bump(),
            name: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            folders: Vec::new(),
            messages: Vec::new(),
        };
        for path in entries {
            if path.is_dir() {
                if depth < MAX_DIRECTORY_DEPTH {
                    self.census.subdirectories_mirrored += 1;
                    let sub = self.folder_from_dir(&path, depth + 1)?;
                    folder.folders.push(sub);
                }
            } else if is_msg_file(&path) {
                if let Some(message) = self.message_from_file(&path) {
                    folder.messages.push(message);
                }
            }
        }
        Ok(folder)
    }

    fn message_from_file(&mut self, path: &Path) -> Option<SourceMessage> {
        match cfb::open(path) {
            Ok(mut comp) => {
                let message =
                    self.read_message(&mut comp, Path::new("/"), OxmsgEntryScope::Message, 0);
                self.files.insert(message.id, path.to_path_buf());
                Some(message)
            }
            Err(_) => {
                self.census.open_errors += 1;
                None
            }
        }
    }

    /// Reads the message whose own streams live directly under `base` (`/` for the file's
    /// message, an embedded-object storage for a nested one).
    fn read_message(
        &mut self,
        comp: &mut CompoundFile,
        base: &Path,
        scope: OxmsgEntryScope,
        depth: usize,
    ) -> SourceMessage {
        let header_len = properties_stream_header_len(scope).unwrap_or(32);
        let codepage = extract_string8_codepage(comp, base, header_len).codepage;

        let subject = read_string_property(comp, base, PROP_SUBJECT, codepage).map(|raw| {
            let (text, marker) = strip_subject_marker(&raw);
            if marker {
                self.census.subject_markers_stripped += 1;
            }
            text.to_string()
        });
        if depth == 0 {
            let internet_id = read_string_property(comp, base, PROP_INTERNET_MESSAGE_ID, codepage);
            self.ids.note(internet_id.as_deref(), &mut self.census);
        }
        let time = message_time(comp, base, header_len);
        let id = self.bump();
        let attachments = self.read_attachments(comp, base, codepage, depth);
        SourceMessage {
            id,
            subject,
            time,
            attachments,
        }
    }

    fn read_attachments(
        &mut self,
        comp: &mut CompoundFile,
        base: &Path,
        codepage: u32,
        depth: usize,
    ) -> Vec<SourceAttachment> {
        let mut paths: Vec<PathBuf> = comp
            .walk()
            .filter(|e| {
                e.path().parent() == Some(base)
                    && matches!(
                        classify_oxmsg_entry(&cfb_entry_name(e.path()), e.is_root()),
                        OxmsgEntryKind::AttachmentStorage
                    )
            })
            .map(|e| e.path().to_path_buf())
            .collect();
        paths.sort();

        let mut attachments = Vec::with_capacity(paths.len());
        for attach_path in paths {
            let method = attach_method(comp, &attach_path);
            let embedded_path = attach_path.join(EMBEDDED_OBJECT_STORAGE_NAME);
            let is_nested_message = method == Some(ATTACH_METHOD_EMBEDDED)
                && depth < MAX_EMBEDDED_READ_DEPTH
                && read_stream_bytes(comp, &embedded_path.join("__properties_version1.0"))
                    .is_some();
            let embedded = if is_nested_message {
                Some(self.read_message(
                    comp,
                    &embedded_path,
                    OxmsgEntryScope::EmbeddedObject,
                    depth + 1,
                ))
            } else {
                None
            };

            let name =
                read_string_property(comp, &attach_path, PROP_ATTACH_LONG_FILENAME, codepage)
                    .or_else(|| {
                        read_string_property(comp, &attach_path, PROP_ATTACH_FILENAME, codepage)
                    })
                    .filter(|n| !n.trim().is_empty());
            if embedded.is_none() && name.is_none() {
                self.census.attachments_without_name += 1;
            }
            let id = self.bump();
            attachments.push(SourceAttachment { id, name, embedded });
        }
        attachments
    }
}

/// Reads what the export writes for one top-level message (M4c): subject, Internet message ID,
/// time, the plain-text body, and which other body forms exist. Content stays in memory and goes
/// only into the archive directory the user named.
pub(crate) fn read_message_content(path: &Path) -> Result<MessageContent> {
    let mut comp = cfb::open(path).context("failed to open a .msg file for export")?;
    let base = Path::new("/");
    let header_len = properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32);
    let codepage = extract_string8_codepage(&mut comp, base, header_len).codepage;

    let subject = read_string_property(&mut comp, base, PROP_SUBJECT, codepage)
        .map(|raw| strip_subject_marker(&raw).0.to_string());
    let internet_message_id =
        read_string_property(&mut comp, base, PROP_INTERNET_MESSAGE_ID, codepage)
            .filter(|id| !id.trim().is_empty());
    let time_filetime = message_time(&mut comp, base, header_len);
    let plain_body = match read_string_property(&mut comp, base, PROP_BODY, codepage) {
        None => PlainBody::Absent,
        Some(text) if text.trim().is_empty() => PlainBody::Empty,
        Some(text) => PlainBody::Text(text),
    };
    let flags = extract_body_flags(&mut comp);
    let envelope = extract_envelope(&mut comp);

    Ok(MessageContent {
        subject,
        internet_message_id,
        time_filetime,
        plain_body,
        bodies: BodyAvailability {
            html_native: flags.has_html_native,
            html_via_rtf: flags.has_html_via_rtf,
            rtf: flags.has_rtf,
        },
        envelope,
    })
}

/// Delivery time, falling back to submit time, as FILETIME ticks.
fn message_time(comp: &mut CompoundFile, base: &Path, header_len: usize) -> Option<i64> {
    let bytes = read_stream_bytes(comp, &base.join("__properties_version1.0"))?;
    let decoded = decode_properties_stream(&bytes, header_len)?;
    let find = |wanted: u16| -> Option<i64> {
        decoded.entries.iter().find_map(|entry| {
            if entry.property_id != wanted || entry.property_type != 0x0040 {
                return None;
            }
            match decode_fixed_value(entry.property_type, &entry.tail) {
                Some(DecodedFixedValue::SysTime(ticks)) => i64::try_from(ticks).ok(),
                _ => None,
            }
        })
    };
    find(PROP_DELIVERY_TIME).or_else(|| find(PROP_SUBMIT_TIME))
}

fn attach_method(comp: &mut CompoundFile, attach_path: &Path) -> Option<i32> {
    let bytes = read_stream_bytes(comp, &attach_path.join("__properties_version1.0"))?;
    let header_len = properties_stream_header_len(OxmsgEntryScope::Attachment).unwrap_or(8);
    let decoded = decode_properties_stream(&bytes, header_len)?;
    decoded.entries.iter().find_map(|entry| {
        if entry.property_id != PROP_ATTACH_METHOD || entry.property_type != 0x0003 {
            return None;
        }
        match decode_fixed_value(entry.property_type, &entry.tail) {
            Some(DecodedFixedValue::Long(v)) => Some(v),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{plan_export, verify_plan, Policy};
    use std::io::Write;

    fn entry(ty: u16, id: u16, value: [u8; 8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&value);
        v
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    /// A message with a marker-prefixed subject, a delivery time, a plain-text body, one file
    /// attachment, and one embedded message.
    fn write_synthetic_msg(path: &Path) {
        let mut comp = cfb::create(path).expect("create CFB");
        let ticks: u64 = 133_000_000_000_000_000;
        let mut props = vec![0u8; 32];
        props.extend(entry(0x0040, PROP_DELIVERY_TIME, ticks.to_le_bytes()));
        put(&mut comp, "/__properties_version1.0", &props);
        put(
            &mut comp,
            "/__substg1.0_0037001F",
            &utf16("\u{1}\u{4}RE: Hello"),
        );
        put(&mut comp, "/__substg1.0_1000001F", &utf16("Body text"));

        comp.create_storage("/__attach_version1.0_#00000000")
            .expect("create attachment storage");
        let mut a0 = vec![0u8; 8];
        a0.extend(entry(0x0003, PROP_ATTACH_METHOD, [1, 0, 0, 0, 0, 0, 0, 0]));
        put(
            &mut comp,
            "/__attach_version1.0_#00000000/__properties_version1.0",
            &a0,
        );
        put(
            &mut comp,
            "/__attach_version1.0_#00000000/__substg1.0_3707001F",
            &utf16("report.pdf"),
        );

        comp.create_storage("/__attach_version1.0_#00000001")
            .expect("create attachment storage");
        let mut a1 = vec![0u8; 8];
        a1.extend(entry(0x0003, PROP_ATTACH_METHOD, [5, 0, 0, 0, 0, 0, 0, 0]));
        put(
            &mut comp,
            "/__attach_version1.0_#00000001/__properties_version1.0",
            &a1,
        );
        comp.create_storage("/__attach_version1.0_#00000001/__substg1.0_3701000D")
            .expect("create embedded storage");
        put(
            &mut comp,
            "/__attach_version1.0_#00000001/__substg1.0_3701000D/__properties_version1.0",
            &[0u8; 24],
        );
        put(
            &mut comp,
            "/__attach_version1.0_#00000001/__substg1.0_3701000D/__substg1.0_0037001F",
            &utf16("Inner"),
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-src-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_synthetic_message_is_read_with_subject_time_and_attachments() {
        let dir = temp_dir("one");
        let file = dir.join("m.msg");
        write_synthetic_msg(&file);

        let (tree, census) = build_msg_tree(&file).expect("build tree");
        assert_eq!(tree.messages.len(), 1);
        let m = &tree.messages[0];
        assert_eq!(m.subject.as_deref(), Some("RE: Hello"));
        assert_eq!(m.time, Some(133_000_000_000_000_000));
        assert_eq!(census.subject_markers_stripped, 1);
        assert_eq!(census.internet_id_missing, 1);
        assert_eq!(m.attachments.len(), 2);
        assert_eq!(m.attachments[0].name.as_deref(), Some("report.pdf"));
        assert!(m.attachments[0].embedded.is_none());
        let inner = m.attachments[1]
            .embedded
            .as_ref()
            .expect("embedded message");
        assert_eq!(inner.subject.as_deref(), Some("Inner"));
        assert_eq!(census.attachments_without_name, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_mirrors_its_subdirectories_and_plans_cleanly() {
        let dir = temp_dir("tree");
        std::fs::create_dir_all(dir.join("sub")).expect("create subdirectory");
        write_synthetic_msg(&dir.join("a.msg"));
        write_synthetic_msg(&dir.join("sub").join("b.msg"));
        std::fs::write(dir.join("notes.txt"), b"not a message").expect("write other file");
        std::fs::write(dir.join("broken.msg"), b"not a compound file").expect("write broken");

        let (tree, census) = build_msg_tree(&dir).expect("build tree");
        assert_eq!(census.subdirectories_mirrored, 1);
        assert_eq!(census.open_errors, 1);
        assert_eq!(tree.messages.len(), 1);
        assert_eq!(tree.folders.len(), 1);
        assert_eq!(tree.folders[0].messages.len(), 1);

        let policy = Policy::new(40);
        let plan = plan_export(&tree, &policy);
        assert_eq!(verify_plan(&plan, &policy).violations(), 0);
        // Two identical subjects in different folders do not collide; within one folder the
        // message directory holds an attachments folder with the file and the embedded message.
        let names: Vec<String> = plan
            .entries
            .iter()
            .map(|e| e.components.join("/"))
            .collect();
        assert!(names.contains(&"RE_ Hello".to_string()));
        assert!(names.contains(&"RE_ Hello/attachments/report.pdf".to_string()));
        assert!(names.contains(&"RE_ Hello/attachments/Inner".to_string()));
        assert!(names.contains(&"sub".to_string()));
        assert!(names.contains(&"sub/RE_ Hello".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_top_level_message_is_mapped_to_its_file() {
        let dir = temp_dir("files");
        std::fs::create_dir_all(dir.join("sub")).expect("create subdirectory");
        write_synthetic_msg(&dir.join("a.msg"));
        write_synthetic_msg(&dir.join("sub").join("b.msg"));

        let (tree, _census, files) = build_msg_export_source(&dir).expect("build source");
        assert_eq!(files.len(), 2);
        let top = tree.messages[0].id;
        let nested = tree.folders[0].messages[0].id;
        assert_eq!(files.get(&top), Some(&dir.join("a.msg")));
        assert_eq!(files.get(&nested), Some(&dir.join("sub").join("b.msg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn message_content_is_read_for_export() {
        let dir = temp_dir("content");
        let file = dir.join("m.msg");
        write_synthetic_msg(&file);

        let content = read_message_content(&file).expect("read content");
        assert_eq!(content.subject.as_deref(), Some("RE: Hello"));
        assert_eq!(content.time_filetime, Some(133_000_000_000_000_000));
        assert_eq!(content.internet_message_id, None);
        assert_eq!(content.plain_body, PlainBody::Text("Body text".to_string()));
        assert_eq!(
            content.envelope.delivery_time,
            Some(133_000_000_000_000_000)
        );
        assert_eq!(content.envelope.submit_time, None);
        assert_eq!(content.envelope.sender, None);
        assert!(content.envelope.recipients.is_empty());
        assert!(!content.bodies.html_native);
        assert!(!content.bodies.html_via_rtf);
        assert!(!content.bodies.rtf);

        std::fs::write(dir.join("broken.msg"), b"not a compound file").expect("write broken");
        assert!(read_message_content(&dir.join("broken.msg")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
