//! The PST side of the export (M4i, decisions D1 to D5).
//!
//! [`PstSource`] holds the open Unicode store. It builds the planner's source tree (through the
//! same builder the dry run uses, so the plan and the export read one store), opens one message
//! at a time for staging, and gives each message a [`PstAttachments`] that streams its
//! by-value attachments from the crate's buffer. An ANSI PST is refused (no ANSI fixture), a
//! password-protected PST is refused unless its password verifies, and every call into
//! `outlook-pst` for a message runs inside `run_contained` so a panic in the crate becomes an
//! error instead of ending the run.
//!
//! Containment nests here: `read_message` runs inside `run_contained` and calls
//! `decompress_rtf_bounded`, which uses `run_contained` too. That is safe only because
//! `run_contained` is reentrant on one thread (`src/shared.rs`); with the earlier plain mutex the
//! first export of a message with an RTF body deadlocked (2026-11-09). It does not extend across
//! threads: never call `run_contained` from a thread that this code spawns and then waits for.
//!
//! What a message carries here (drop A): subject, Internet message ID, time, the body forms as
//! the source stores them (plain text, native HTML as `PT_BINARY` only, decompressed RTF, the
//! Internet code page), and a descriptor for each attachment. The envelope arrives in drop B
//! and is empty until then. Attachment bytes are never read to build a descriptor, except to
//! read the content ID of a by-value attachment from a table that has no content-ID column
//! (the attachment is opened, its content ID taken, and its bytes dropped at once).
//!
//! Embedded-message and OLE attachments cannot be read through the crate (the read fails with
//! `AttachmentMethodNotFound`), so no reader here ever opens them: the exporter records them as
//! not extracted from their method alone.

use std::io::Cursor;
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::messaging::attachment::{
    Attachment as PstAttachment, AttachmentData, UnicodeAttachment,
};
use outlook_pst::messaging::message::{Message as PstMessage, UnicodeMessage};
use outlook_pst::messaging::store::{Store, UnicodeStore};
use outlook_pst::ndb::node_id::NodeId;

use crate::archive::PstIdentity;
use crate::dry_run::{PROP_INTERNET_MESSAGE_ID, PROP_SUBJECT, SourceCensus, strip_subject_marker};
use crate::export_source::DataOpen;
use crate::model::{AttachmentInfo, BodyForms, Envelope, MessageContent};
use crate::oxmsg_decode::{PROP_INTERNET_CODEPAGE, PROP_MESSAGE_CODEPAGE};
use crate::plan::SourceFolder;
use crate::pst_attachments::{
    AttachmentRow, attachment_rows, error_label, is_missing_data_label, open_attachment,
    open_unicode_pst,
};
use crate::pst_password::{PasswordOutcome, enforce, supplied_from_env};
use crate::pst_text::{TextReader, TextStats, integer32};
use crate::shared::{
    ATTACH_METHOD_BY_VALUE, PROP_ATTACH_CONTENT_ID, PROP_BODY, PROP_BODY_HTML, PROP_RTF_COMPRESSED,
    decompress_rtf_bounded, run_contained,
};
use crate::source_pst::{build_pst_tree_from, message_time};
use crate::stage::hex;

/// Content-free counts gathered while reading messages for the export.
#[derive(Debug, Clone, Default)]
pub(crate) struct PstNotes {
    /// `PT_STRING8` values read and what was lost reading them.
    pub(crate) text: TextStats,
    /// Messages whose `PidTagHtml` was present but not `PT_BINARY` (not read).
    pub(crate) html_not_binary: u64,
    /// Messages whose `PidTagRtfCompressed` was present but not `PT_BINARY` (not read).
    pub(crate) rtf_not_binary: u64,
}

/// An open message: the content the export writes, a reader for its attachments, and its
/// identity in the store.
pub(crate) struct OpenPstMessage {
    pub(crate) content: MessageContent,
    pub(crate) attachments: PstAttachments,
    pub(crate) identity: PstIdentity,
}

/// The open Unicode store an export reads from.
pub(crate) struct PstSource {
    store: Rc<UnicodeStore>,
}

impl PstSource {
    /// Opens `path` as a Unicode PST and enforces the password rule (a refusal is a typed error
    /// that ends the run with exit status 2 before anything is staged). An ANSI PST is refused.
    pub(crate) fn open(path: &Path) -> Result<(PstSource, PasswordOutcome)> {
        let pst = open_unicode_pst(path)?;
        let store = UnicodeStore::read(Rc::new(pst)).context("failed to read the PST store")?;
        let outcome = enforce(&*store, supplied_from_env()?)?;
        Ok((PstSource { store }, outcome))
    }

    /// The planner's source tree for the IPM subtree, and the census of reading it.
    pub(crate) fn build_tree(
        &self,
        outcome: PasswordOutcome,
    ) -> Result<(SourceFolder, SourceCensus)> {
        build_pst_tree_from(&*self.store, outcome)
    }

    /// The identity of the folder whose node ID is `node_id`: the node ID, the store's provider
    /// UID, and the entry ID.
    pub(crate) fn folder_identity(&self, node_id: u64) -> Result<PstIdentity> {
        let nid = u32::try_from(node_id).context("a PST folder ID does not fit 32 bits")?;
        let entry_id = self
            .store
            .properties()
            .make_entry_id(NodeId::from(nid))
            .context("failed to build a PST folder ID")?;
        Ok(PstIdentity {
            node_id: nid,
            provider_uid: hex(entry_id.record_key()),
            entry_id: Some(entry_id_hex(entry_id.record_key(), nid)),
        })
    }

    /// Opens the message whose node ID is `node_id`. The read runs inside `run_contained`, and
    /// the RTF decompression inside it nests another `run_contained` on the same thread (see the
    /// module comment).
    pub(crate) fn open_message(
        &self,
        node_id: u64,
        notes: &mut PstNotes,
    ) -> Result<OpenPstMessage> {
        let nid = u32::try_from(node_id).context("a PST message ID does not fit 32 bits")?;
        run_contained(|| self.read_message(NodeId::from(nid), notes)).unwrap_or_else(|| {
            Err(anyhow!(
                "a panic in the PST reader was contained while reading a message"
            ))
        })
    }

    fn read_message(&self, node: NodeId, notes: &mut PstNotes) -> Result<OpenPstMessage> {
        let entry_id = self
            .store
            .properties()
            .make_entry_id(node)
            .context("failed to build a PST message ID")?;
        let message = UnicodeMessage::read(Rc::clone(&self.store), &entry_id, None)
            .context("failed to open a PST message for export")?;
        let properties = message.properties();

        let mut text = TextReader::new();
        text.use_message_codepage(
            integer32(properties.get(PROP_MESSAGE_CODEPAGE)),
            integer32(properties.get(PROP_INTERNET_CODEPAGE)),
        );

        let subject = properties
            .get(PROP_SUBJECT)
            .and_then(|value| text.text(value))
            .map(|raw| strip_subject_marker(&raw).0.to_string());
        let internet_message_id = properties
            .get(PROP_INTERNET_MESSAGE_ID)
            .and_then(|value| text.text(value))
            .filter(|id| !id.trim().is_empty());
        let time_filetime = message_time(&*message);

        // Body forms as stored. A zero-length plain body stays `Some("")`; a binary form that
        // is empty is absent. Native HTML and RTF are read as `PT_BINARY` only.
        let plain_text = properties.get(PROP_BODY).and_then(|value| text.text(value));
        let native_html = match properties.get(PROP_BODY_HTML) {
            Some(PropertyValue::Binary(value)) => {
                Some(value.buffer().to_vec()).filter(|bytes| !bytes.is_empty())
            }
            Some(_) => {
                notes.html_not_binary += 1;
                None
            }
            None => None,
        };
        let native_html_code_page = integer32(properties.get(PROP_INTERNET_CODEPAGE))
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0);
        let (rtf, rtf_decompression_failed) = match properties.get(PROP_RTF_COMPRESSED) {
            Some(PropertyValue::Binary(value)) if !value.buffer().is_empty() => {
                match decompress_rtf_bounded(value.buffer()) {
                    Some(bytes) if bytes.is_empty() => (None, false),
                    Some(bytes) => (Some(bytes), false),
                    None => (None, true),
                }
            }
            Some(PropertyValue::Binary(_)) | None => (None, false),
            Some(_) => {
                notes.rtf_not_binary += 1;
                (None, false)
            }
        };
        let bodies = BodyForms {
            native_html,
            native_html_code_page,
            rtf,
            rtf_decompression_failed,
            plain_text,
        };

        // One descriptor per attachment row, in table order: the order of the planner's source
        // attachments.
        let rows = attachment_rows(&*message, &mut text).unwrap_or_default();
        let mut attachments = Vec::with_capacity(rows.len());
        for row in &rows {
            let content_id = if row.content_id_column || row.method != Some(ATTACH_METHOD_BY_VALUE)
            {
                row.content_id.clone()
            } else {
                content_id_from_attachment(&message, row.node, &mut text)
            };
            attachments.push(AttachmentInfo {
                method: row.method,
                content_id,
                data_len: None,
            });
        }

        notes.text.add(&text.stats);
        let identity = PstIdentity {
            node_id: u32::from(node),
            provider_uid: hex(entry_id.record_key()),
            entry_id: None,
        };
        Ok(OpenPstMessage {
            content: MessageContent {
                subject,
                internet_message_id,
                time_filetime,
                bodies,
                envelope: Envelope::default(),
                attachments,
            },
            attachments: PstAttachments {
                message,
                rows,
                current: None,
            },
            identity,
        })
    }
}

/// The content ID of a by-value attachment, read from the attachment itself (its bytes are
/// dropped as soon as this returns). `None` when it cannot be opened or has none.
fn content_id_from_attachment(
    message: &Rc<UnicodeMessage>,
    node: NodeId,
    text: &mut TextReader,
) -> Option<String> {
    let attachment = open_attachment(message, node).ok()?;
    attachment
        .properties()
        .get(PROP_ATTACH_CONTENT_ID)
        .and_then(|value| text.text(value))
        .filter(|id| !id.trim().is_empty())
}

/// An entry ID as MS-PST lays it out and the crate writes it: four zero flag bytes, the
/// 16-byte provider UID, and the node ID little-endian, as lower-case hex.
fn entry_id_hex(provider_uid: &[u8; 16], node_id: u32) -> String {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(provider_uid);
    bytes.extend_from_slice(&node_id.to_le_bytes());
    hex(&bytes)
}

/// Streams one message's by-value attachments by position. Holds the attachment last opened
/// (the crate's buffer, about twice the payload while it lives) until the next one is opened or
/// the message is done; it never holds more than one.
pub(crate) struct PstAttachments {
    message: Rc<UnicodeMessage>,
    rows: Vec<AttachmentRow>,
    current: Option<Rc<UnicodeAttachment>>,
}

impl PstAttachments {
    /// The data of attachment `index`. A null or absent data property is `Missing`; any other
    /// failure to read is `ReadFailed`. Both carry a content-free label of the crate's error.
    pub(crate) fn open_data(&mut self, index: usize) -> DataOpen<'_> {
        let Some(row) = self.rows.get(index) else {
            return DataOpen::Missing { label: None };
        };
        let node = row.node;
        let opened = run_contained(|| open_attachment(&self.message, node));
        let attachment = match opened {
            None => {
                return DataOpen::ReadFailed {
                    label: Some("PanicContained".to_string()),
                };
            }
            Some(Err(err)) => {
                let label = error_label(&err);
                return if is_missing_data_label(&label) {
                    DataOpen::Missing { label: Some(label) }
                } else {
                    DataOpen::ReadFailed { label: Some(label) }
                };
            }
            Some(Ok(attachment)) => attachment,
        };
        self.current = Some(attachment);
        match self.current.as_ref().and_then(|held| held.data()) {
            Some(AttachmentData::Binary(binary)) => {
                let bytes = binary.buffer();
                DataOpen::Stream {
                    declared: bytes.len() as u64,
                    data: Box::new(Cursor::new(bytes)),
                }
            }
            _ => DataOpen::Missing {
                label: Some("NoBinaryData".to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_id_is_flags_then_provider_uid_then_the_node_id_little_endian() {
        let uid = [0xAAu8; 16];
        let id = entry_id_hex(&uid, 0x0020_1122);
        assert_eq!(id.len(), 48);
        assert_eq!(&id[..8], "00000000");
        assert_eq!(&id[8..40], "aa".repeat(16));
        assert_eq!(&id[40..], "22112000");
    }

    #[test]
    fn the_notes_start_at_zero() {
        let notes = PstNotes::default();
        assert_eq!(notes.html_not_binary, 0);
        assert_eq!(notes.rtf_not_binary, 0);
        assert_eq!(notes.text, TextStats::default());
    }
}
