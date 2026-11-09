//! The concrete route from a PST message to its attachments' payloads (M4i step 0).
//!
//! `outlook-pst` 1.2.0's dyn `Store` and `Message` API cannot reach an attachment. The concrete
//! Unicode types can: `UnicodeMessage::read`, then `UnicodeAttachment::read` with the node ID
//! taken from the attachment-table row ID, then `data()`. Everything that touches that route,
//! and the table plumbing around it, lives here so that the spike and the later exporter share
//! one reading path.
//!
//! Unicode PSTs only. An ANSI store needs a parallel path (`AnsiAttachment`) and has no fixture.
//! Nothing here prints; error labels carry a variant name and never a payload, so they are safe
//! to count.

use std::io;
use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use outlook_pst::ltp::table_context::TableContext;
use outlook_pst::messaging::attachment::UnicodeAttachment;
use outlook_pst::messaging::folder::Folder as PstFolder;
use outlook_pst::messaging::message::{Message as PstMessage, UnicodeMessage};
use outlook_pst::ndb::node_id::NodeId;
use outlook_pst::{AnsiPstFile, UnicodePstFile};

use crate::pst::{column_index, read_i32_at};
use crate::pst_text::{TextReader, read_text_at};
use crate::shared::{
    ATTACH_METHOD_BY_REFERENCE, ATTACH_METHOD_BY_REFERENCE_ONLY,
    ATTACH_METHOD_BY_REFERENCE_RESOLVE, ATTACH_METHOD_BY_VALUE, ATTACH_METHOD_EMBEDDED_MESSAGE,
    ATTACH_METHOD_NONE, ATTACH_METHOD_OLE, PROP_ATTACH_CONTENT_ID, PROP_ATTACH_METHOD,
    PROP_ATTACH_SIZE,
};

/// One row of a message's attachment table, as far as it could be read.
pub(crate) struct AttachmentRow {
    /// The row ID, which the crate takes as the attachment's sub-node ID.
    pub(crate) node: NodeId,
    /// `PidTagAttachMethod` from the row, when present and a 32-bit integer.
    pub(crate) method: Option<i32>,
    /// `PidTagAttachSize` from the row, when present and a 32-bit integer.
    pub(crate) size: Option<i32>,
    /// True when the row's column values could not be read at all (`method`, `size`, and
    /// `content_id` are then `None`).
    pub(crate) columns_unreadable: bool,
    /// `PidTagAttachContentId` from the row, when the table has that column and the value is
    /// not blank.
    pub(crate) content_id: Option<String>,
    /// True when the table has a content-ID column at all. When it does not, the content ID can
    /// only be read from the attachment itself.
    pub(crate) content_id_column: bool,
}

/// The attachment-table rows of `message`, in table order, or `None` when the message has no
/// attachment table. Text columns are read through `text`, under the code page it holds.
pub(crate) fn attachment_rows(
    message: &dyn PstMessage,
    text: &mut TextReader,
) -> Option<Vec<AttachmentRow>> {
    let table_rc = message.attachment_table()?;
    let table: &dyn TableContext = table_rc.as_ref();
    let context = table.context();
    let size_idx = column_index(context, PROP_ATTACH_SIZE);
    let method_idx = column_index(context, PROP_ATTACH_METHOD);
    let content_idx = column_index(context, PROP_ATTACH_CONTENT_ID);

    Some(
        table
            .rows_matrix()
            .map(|row| {
                let node = NodeId::from(u32::from(row.id()));
                match row.columns(context) {
                    Ok(values) => AttachmentRow {
                        node,
                        method: method_idx.and_then(|i| read_i32_at(table, context, &values, i)),
                        size: size_idx.and_then(|i| read_i32_at(table, context, &values, i)),
                        columns_unreadable: false,
                        content_id: content_idx
                            .and_then(|i| read_text_at(table, context, &values, i, text))
                            .filter(|id| !id.trim().is_empty()),
                        content_id_column: content_idx.is_some(),
                    },
                    Err(_) => AttachmentRow {
                        node,
                        method: None,
                        size: None,
                        columns_unreadable: true,
                        content_id: None,
                        content_id_column: content_idx.is_some(),
                    },
                }
            })
            .collect(),
    )
}

/// The number of attachment-table rows of `message`, or `None` when it has no attachment table.
/// Used for an embedded message, whose own attachments cannot be opened through this route.
pub(crate) fn attachment_row_count(message: &dyn PstMessage) -> Option<u64> {
    let table_rc = message.attachment_table()?;
    let table: &dyn TableContext = table_rc.as_ref();
    Some(table.rows_matrix().count() as u64)
}

/// Opens one attachment of a concrete Unicode message. This reads every property of the
/// attachment, the payload included (the crate ignores the property filter for attachments), so
/// a by-value payload is held in memory about twice while the returned object lives.
pub(crate) fn open_attachment(
    message: &Rc<UnicodeMessage>,
    node: NodeId,
) -> io::Result<Rc<UnicodeAttachment>> {
    UnicodeAttachment::read(Rc::clone(message), node, None)
}

/// Opens `path` as a Unicode PST. An ANSI-format PST is told apart from an unreadable file and
/// refused with its own message (there is no ANSI fixture, so there is no ANSI path).
pub(crate) fn open_unicode_pst(path: &Path) -> Result<UnicodePstFile> {
    match UnicodePstFile::open(path) {
        Ok(pst) => Ok(pst),
        Err(err) => {
            if AnsiPstFile::open(path).is_ok() {
                anyhow::bail!(
                    "ANSI-format PST: only Unicode PSTs are supported here (there is no ANSI fixture yet)"
                );
            }
            Err(anyhow::Error::new(err).context("failed to open PST"))
        }
    }
}

/// True for the error labels that mean the attachment has no data to read: its data property is
/// null (an empty value) or missing. Any other label is a read failure.
pub(crate) fn is_missing_data_label(label: &str) -> bool {
    label == "InvalidMessageObjectData(Null)" || label == "AttachmentMessageObjectDataNotFound"
}

/// The node IDs of the messages listed in a folder's contents table, in table order.
pub(crate) fn message_nodes(folder: &dyn PstFolder) -> Vec<NodeId> {
    folder
        .contents_table()
        .map_or_else(Vec::new, |table| table_row_nodes(table.as_ref()))
}

/// The node IDs of the subfolders listed in a folder's hierarchy table, in table order.
pub(crate) fn subfolder_nodes(folder: &dyn PstFolder) -> Vec<NodeId> {
    folder
        .hierarchy_table()
        .map_or_else(Vec::new, |table| table_row_nodes(table.as_ref()))
}

fn table_row_nodes(table: &dyn TableContext) -> Vec<NodeId> {
    table
        .rows_matrix()
        .map(|row| NodeId::from(u32::from(row.id())))
        .collect()
}

/// A fixed, content-free name for an attachment method value.
pub(crate) fn method_label(method: Option<i32>) -> &'static str {
    match method {
        Some(ATTACH_METHOD_NONE) => "none",
        Some(ATTACH_METHOD_BY_VALUE) => "by_value",
        Some(ATTACH_METHOD_BY_REFERENCE) => "by_reference",
        Some(ATTACH_METHOD_BY_REFERENCE_RESOLVE) => "by_reference_resolve",
        Some(ATTACH_METHOD_BY_REFERENCE_ONLY) => "by_reference_only",
        Some(ATTACH_METHOD_EMBEDDED_MESSAGE) => "embedded_message",
        Some(ATTACH_METHOD_OLE) => "ole",
        Some(_) => "other",
        None => "unknown",
    }
}

/// A content-free label for a read error: the name of the crate's error variant, followed by its
/// payload only when that payload is one bare word or number (a property type such as `Null`, a
/// method number), or the kind of a plain I/O error. A quoted, braced, or multi-part payload is
/// never kept, so no text from a file can reach a label. The crate raises some variants whose
/// names do not match the failing arm (the by-value arm uses the embedded-message variants), so
/// a label is only meaningful beside the attachment method.
pub(crate) fn error_label(err: &io::Error) -> String {
    match err.get_ref() {
        Some(inner) => debug_label(&format!("{inner:?}")),
        None => format!("io_{:?}", err.kind()),
    }
}

/// `Name`, or `Name(payload)` when the `Debug` text is exactly `Name(payload)` and the payload is
/// one bare word or number. `unnamed` when the text does not start with a name (a boxed text
/// error starts with a quote, so its text is never kept).
fn debug_label(text: &str) -> String {
    let name = leading_identifier(text);
    if name.is_empty() {
        return "unnamed".to_string();
    }
    let payload = text[name.len()..]
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'));
    match payload {
        Some(word)
            if !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
        {
            format!("{name}({word})")
        }
        _ => name.to_string(),
    }
}

/// The leading run of ASCII letters, digits, and underscores of `text` (possibly empty).
fn leading_identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    enum Kind {
        Null,
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    enum FakeError {
        Empty,
        MissingThing(u32),
        Typed(Kind),
        Named(String),
        Located { index: u32 },
    }

    impl fmt::Display for FakeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fake")
        }
    }

    impl std::error::Error for FakeError {}

    fn label_of(error: FakeError) -> String {
        error_label(&io::Error::new(io::ErrorKind::InvalidData, error))
    }

    #[test]
    fn every_method_value_has_a_fixed_label() {
        let cases = [
            (Some(0), "none"),
            (Some(1), "by_value"),
            (Some(2), "by_reference"),
            (Some(3), "by_reference_resolve"),
            (Some(4), "by_reference_only"),
            (Some(5), "embedded_message"),
            (Some(6), "ole"),
            (Some(7), "other"),
            (Some(-1), "other"),
            (None, "unknown"),
        ];
        for (method, label) in cases {
            assert_eq!(method_label(method), label, "method {method:?}");
        }
    }

    #[test]
    fn a_bare_word_or_number_payload_is_kept() {
        assert_eq!(label_of(FakeError::Typed(Kind::Null)), "Typed(Null)");
        assert_eq!(label_of(FakeError::MissingThing(7)), "MissingThing(7)");
        assert_eq!(label_of(FakeError::Empty), "Empty");
    }

    #[test]
    fn a_text_or_structured_payload_is_never_kept() {
        assert_eq!(
            label_of(FakeError::Named("secret subject line".to_string())),
            "Named"
        );
        assert_eq!(label_of(FakeError::Located { index: 3 }), "Located");
    }

    #[test]
    fn a_boxed_text_error_is_never_kept() {
        let err = io::Error::new(io::ErrorKind::InvalidData, "secret subject line");
        assert_eq!(error_label(&err), "unnamed");
    }

    #[test]
    fn only_a_null_or_missing_data_property_counts_as_missing_data() {
        assert!(is_missing_data_label("InvalidMessageObjectData(Null)"));
        assert!(is_missing_data_label("AttachmentMessageObjectDataNotFound"));
        assert!(!is_missing_data_label("InvalidMessageObjectData(Binary)"));
        assert!(!is_missing_data_label("AttachmentMethodNotFound"));
        assert!(!is_missing_data_label("io_UnexpectedEof"));
    }

    #[test]
    fn a_plain_io_error_is_labelled_by_its_kind() {
        let err = io::Error::from(io::ErrorKind::UnexpectedEof);
        assert_eq!(error_label(&err), "io_UnexpectedEof");
    }

    #[test]
    fn the_debug_label_keeps_only_a_name_and_one_bare_payload() {
        let cases = [
            ("Name", "Name"),
            ("Name(Null)", "Name(Null)"),
            ("Name(3)", "Name(3)"),
            ("Name(-1)", "Name(-1)"),
            ("Name()", "Name"),
            ("Name(12, \"x\")", "Name"),
            ("Name(\"x\")", "Name"),
            ("Name { a: 1 }", "Name"),
            ("Name(Null) tail", "Name"),
            ("Name(a b)", "Name"),
            ("\"quoted\"", "unnamed"),
            ("", "unnamed"),
        ];
        for (text, label) in cases {
            assert_eq!(debug_label(text), label, "text {text:?}");
        }
    }
}
