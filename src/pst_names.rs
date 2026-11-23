//! Attachment names from the attachment objects (names fix, release 0.1.46).
//!
//! An attachment table of a PST need not carry the long file name, only a short (8.3) one. On
//! `crkb.pst` just 464 of 2,854 attachment tables had a long-name column, and for the other rows
//! the planner's name was the short name: an 8.3 alias (`SCREEN~1.JPG`), another truncated form,
//! or nothing, in 4,210 of 7,016 rows (the name spike, 2026-11-22). Every attachment object
//! carried the real name (`PidTagAttachLongFilename`), so the planner asks the object whenever
//! the table gives no long name. It asks for a by-value attachment only, because the crate cannot
//! read the object of an embedded-message or OLE attachment (see `source_pst.rs`). The table's
//! long name wins when it has one; if the object has no long name or cannot be read, the table's
//! name is kept and the outcome is counted (`SourceCensus`), so nothing is silent.
//!
//! Names must be settled while the tree is built: collision suffixes, the path budget, and the
//! `cid:` links of the exported bodies all derive from them. Reading an object reads its payload
//! (the crate ignores the property filter for attachments), one attachment at a time, so memory
//! does not grow; opening every object on `crkb.pst` took about 14 s on a warm cache (the spike).

use std::path::Path;
use std::rc::Rc;

use outlook_pst::messaging::attachment::Attachment as PstAttachment;
use outlook_pst::messaging::message::{Message as PstMessage, UnicodeMessage};
use outlook_pst::messaging::store::{Store, UnicodeStore};
use outlook_pst::ndb::node_id::NodeId;

use crate::dry_run::PROP_ATTACH_LONG_FILENAME;
use crate::oxmsg_decode::{PROP_INTERNET_CODEPAGE, PROP_MESSAGE_CODEPAGE};
use crate::pst_attachments::{open_attachment, open_unicode_pst};
use crate::pst_text::{TextReader, TextStats, integer32};
use crate::shared::{contained, run_contained};

/// What the attachment object said about its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NameFromObject {
    /// The planner did not ask: the table had a long name, the row is not a by-value
    /// attachment, or there is no reader.
    NotAsked,
    /// The object's `PidTagAttachLongFilename`.
    Found(String),
    /// The object was read and carries no long file name.
    NoLongName,
    /// The object could not be read (an error, or a panic that was contained).
    Unreadable,
}

/// Reads the long file name stored on an attachment object.
pub(crate) trait AttachmentNames {
    /// The long file name of attachment `attachment` of the message with node ID `message_nid`.
    fn long_name(&mut self, message_nid: u32, attachment: NodeId) -> NameFromObject;

    /// What reading the names found in `PT_STRING8` values (the planner adds it to its own).
    fn text_stats(&self) -> TextStats;
}

/// Chooses the planned name of one attachment. The table's long-name value wins when it has one;
/// otherwise `ask_object` is asked once and its long name is used. If the object has none, or
/// cannot be read, the table's name is kept exactly as before 0.1.46: the long-name column's
/// value (so a blank one gives no name), else the short-name column's, and blank is no name.
/// Returns the name and what the object said.
pub(crate) fn pick_name(
    table_long: Option<String>,
    table_short: Option<String>,
    ask_object: impl FnOnce() -> NameFromObject,
) -> (Option<String>, NameFromObject) {
    if table_long
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
    {
        return (table_long, NameFromObject::NotAsked);
    }
    match ask_object() {
        NameFromObject::Found(name) => (Some(name.clone()), NameFromObject::Found(name)),
        said => {
            let kept = table_long
                .or(table_short)
                .filter(|name| !name.trim().is_empty());
            (kept, said)
        }
    }
}

/// Reads attachment names from the attachment objects of one open Unicode store.
pub(crate) struct ObjectNames {
    store: Rc<UnicodeStore>,
    /// Decodes the names, under the code page of the message being read.
    text: TextReader,
    /// The message read last, kept for the rest of its attachments.
    open: Option<(u32, Rc<UnicodeMessage>)>,
}

impl ObjectNames {
    pub(crate) fn new(store: Rc<UnicodeStore>) -> Self {
        ObjectNames {
            store,
            text: TextReader::new(),
            open: None,
        }
    }

    /// The message with node ID `nid`, read once and kept while its attachments are read.
    fn message(&mut self, nid: u32) -> Option<Rc<UnicodeMessage>> {
        if let Some((open_nid, message)) = &self.open
            && *open_nid == nid
        {
            return Some(Rc::clone(message));
        }
        let entry_id = self
            .store
            .properties()
            .make_entry_id(NodeId::from(nid))
            .ok()?;
        let message = UnicodeMessage::read(Rc::clone(&self.store), &entry_id, None).ok()?;
        let properties = message.properties();
        self.text.use_message_codepage(
            integer32(properties.get(PROP_MESSAGE_CODEPAGE)),
            integer32(properties.get(PROP_INTERNET_CODEPAGE)),
        );
        self.open = Some((nid, Rc::clone(&message)));
        Some(message)
    }

    fn read_long_name(&mut self, nid: u32, attachment: NodeId) -> Option<NameFromObject> {
        let message = self.message(nid)?;
        let object = open_attachment(&message, attachment).ok()?;
        let name = object
            .properties()
            .get(PROP_ATTACH_LONG_FILENAME)
            .and_then(|value| self.text.text(value))
            .filter(|text| !text.trim().is_empty());
        Some(match name {
            Some(name) => NameFromObject::Found(name),
            None => NameFromObject::NoLongName,
        })
    }
}

impl AttachmentNames for ObjectNames {
    fn long_name(&mut self, message_nid: u32, attachment: NodeId) -> NameFromObject {
        run_contained(|| self.read_long_name(message_nid, attachment))
            .flatten()
            .unwrap_or(NameFromObject::Unreadable)
    }

    fn text_stats(&self) -> TextStats {
        self.text.stats.clone()
    }
}

/// The attachment-name reader for the Unicode PST at `path`, opened as the export opens it.
/// `None` for a PST that cannot be opened that way: an ANSI PST keeps the names in its
/// attachment tables.
pub(crate) fn names_from_objects(path: &Path) -> Option<Box<dyn AttachmentNames>> {
    let pst = open_unicode_pst(path).ok()?;
    let store = contained(|| UnicodeStore::read(Rc::new(pst)), "reading the store")
        .ok()?
        .ok()?;
    Some(Box::new(ObjectNames::new(store)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_asked() -> NameFromObject {
        panic!("the object must not be asked when the table has a long name");
    }

    #[test]
    fn a_table_long_name_wins_and_the_object_is_not_asked() {
        let (name, said) = pick_name(
            Some("Report 2023.pdf".to_string()),
            Some("REPORT~1.PDF".to_string()),
            never_asked,
        );
        assert_eq!(name.as_deref(), Some("Report 2023.pdf"));
        assert_eq!(said, NameFromObject::NotAsked);
    }

    #[test]
    fn the_objects_long_name_replaces_the_tables_short_name() {
        let found = || NameFromObject::Found("Report 2023.pdf".to_string());
        let (name, said) = pick_name(None, Some("REPORT~1.PDF".to_string()), found);
        assert_eq!(name.as_deref(), Some("Report 2023.pdf"));
        assert_eq!(said, found());
    }

    #[test]
    fn without_a_usable_object_the_tables_short_name_is_kept_and_the_outcome_is_returned() {
        for answer in [
            NameFromObject::NoLongName,
            NameFromObject::Unreadable,
            NameFromObject::NotAsked,
        ] {
            let (name, said) = pick_name(None, Some("a.png".to_string()), || answer.clone());
            assert_eq!(name.as_deref(), Some("a.png"));
            assert_eq!(said, answer);
        }
    }

    #[test]
    fn a_blank_table_long_name_gives_no_name_as_before_but_the_object_is_asked() {
        let (name, said) = pick_name(Some("  ".to_string()), Some("a.png".to_string()), || {
            NameFromObject::Unreadable
        });
        assert_eq!(name, None);
        assert_eq!(said, NameFromObject::Unreadable);
        let (name, _) = pick_name(Some("  ".to_string()), None, || {
            NameFromObject::Found("x.pdf".to_string())
        });
        assert_eq!(name.as_deref(), Some("x.pdf"));
    }

    #[test]
    fn no_name_anywhere_stays_none() {
        let (name, said) = pick_name(None, None, || NameFromObject::NoLongName);
        assert_eq!(name, None);
        assert_eq!(said, NameFromObject::NoLongName);
        let (name, _) = pick_name(None, Some("  ".to_string()), || NameFromObject::Unreadable);
        assert_eq!(name, None);
    }

    #[test]
    fn a_pst_that_cannot_be_opened_has_no_reader() {
        assert!(names_from_objects(Path::new("no-such-file.pst")).is_none());
    }
}
