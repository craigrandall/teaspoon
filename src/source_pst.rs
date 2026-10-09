//! Builds the planner's source tree from a PST (M4b-3): folder identity, display names, message
//! subjects and times, and attachment names from each message's attachment table. Content is
//! read into memory only; nothing is printed.
//!
//! Limits of `outlook-pst` v1.2.0's public API, reported rather than hidden: embedded-message
//! attachments cannot be opened (they are planned as ordinary files and counted), and the
//! attachment table may carry only the short (8.3) file name.

use std::path::Path;

use anyhow::{Context, Result};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::ltp::table_context::{TableContext, TableContextInfo, TableRowColumnValue};
use outlook_pst::messaging::folder::Folder as PstFolder;
use outlook_pst::messaging::message::Message as PstMessage;
use outlook_pst::messaging::store::Store;
use outlook_pst::ndb::node_id::NodeId;

use crate::dry_run::{
    InternetIdTracker, PROP_ATTACH_FILENAME, PROP_ATTACH_LONG_FILENAME, PROP_DELIVERY_TIME,
    PROP_INTERNET_MESSAGE_ID, PROP_SUBJECT, PROP_SUBMIT_TIME, SourceCensus, strip_subject_marker,
};
use crate::plan::{SourceAttachment, SourceFolder, SourceMessage};
use crate::pst::{column_index, read_i32_at};
use crate::shared::{ATTACH_METHOD_EMBEDDED_MESSAGE, PROP_ATTACH_METHOD};

/// PidTagEntryId, injected into every folder's properties by the crate.
const PROP_ENTRY_ID: u16 = 0x0FFF;
/// Deepest folder nesting walked (guards against a corrupt hierarchy).
const MAX_FOLDER_DEPTH: usize = 128;

struct PstBuilder {
    census: SourceCensus,
    ids: InternetIdTracker,
    next_attachment_id: u64,
}

/// Builds the source tree for the IPM subtree of a PST.
pub(crate) fn build_pst_tree(path: &Path) -> Result<(SourceFolder, SourceCensus)> {
    let store = outlook_pst::open_store(path).context("failed to open PST")?;
    let ipm_id = store
        .properties()
        .ipm_sub_tree_entry_id()
        .context("PST has no IPM subtree entry ID")?;
    let ipm = store
        .open_folder(&ipm_id)
        .context("failed to open IPM subtree")?;

    let mut builder = PstBuilder {
        census: SourceCensus::default(),
        ids: InternetIdTracker::default(),
        next_attachment_id: 0,
    };
    let root = builder.folder(store.as_ref(), ipm.as_ref(), 0);
    Ok((root, builder.census))
}

/// Text of a string-typed property value, or `None` for any other type.
fn property_text(value: &PropertyValue) -> Option<String> {
    match value {
        PropertyValue::String8(v) => Some(v.to_string()),
        PropertyValue::Unicode(v) => Some(v.to_string()),
        _ => None,
    }
}

fn read_text_at(
    table: &dyn TableContext,
    context: &TableContextInfo,
    row_values: &[Option<TableRowColumnValue>],
    column_idx: usize,
) -> Option<String> {
    let descriptor = context.columns().get(column_idx)?;
    let value = row_values.get(column_idx)?.as_ref()?;
    property_text(&table.read_column(value, descriptor.prop_type()).ok()?)
}

/// Delivery time, falling back to submit time, as FILETIME ticks.
fn message_time(message: &dyn PstMessage) -> Option<i64> {
    let properties = message.properties();
    [PROP_DELIVERY_TIME, PROP_SUBMIT_TIME]
        .into_iter()
        .find_map(|id| match properties.get(id) {
            Some(PropertyValue::Time(ticks)) => Some(*ticks),
            _ => None,
        })
}

impl PstBuilder {
    fn folder(&mut self, store: &dyn Store, folder: &dyn PstFolder, depth: usize) -> SourceFolder {
        let properties = folder.properties();
        let nid = u32::from(properties.node_id());

        // Folder identity spike: what the crate exposes for a folder.
        if nid == 0 {
            self.census.folder_identity_unavailable += 1;
        } else {
            self.census.folder_identity_nid += 1;
        }
        if properties.get(PROP_ENTRY_ID).is_some() {
            self.census.folder_identity_entry_id += 1;
        }
        let name = match properties.display_name() {
            Ok(name) => name,
            Err(_) => {
                self.census.folder_name_read_errors += 1;
                String::new()
            }
        };

        let mut messages = Vec::new();
        if let Some(contents) = folder.contents_table() {
            for row in contents.rows_matrix() {
                let message_nid = u32::from(row.id());
                let entry_id = match store.properties().make_entry_id(NodeId::from(message_nid)) {
                    Ok(entry_id) => entry_id,
                    Err(_) => {
                        self.census.open_errors += 1;
                        continue;
                    }
                };
                match store.open_message(&entry_id, None) {
                    Ok(message) => {
                        messages.push(self.message(message.as_ref(), u64::from(message_nid)))
                    }
                    Err(_) => self.census.open_errors += 1,
                }
            }
        }
        if let Some(associated) = folder.associated_table() {
            for _ in associated.rows_matrix() {
                self.census.associated_items_skipped += 1;
            }
        }

        let mut folders = Vec::new();
        if depth < MAX_FOLDER_DEPTH
            && let Some(hierarchy) = folder.hierarchy_table()
        {
            for row in hierarchy.rows_matrix() {
                let node = NodeId::from(u32::from(row.id()));
                let entry_id = match store.properties().make_entry_id(node) {
                    Ok(entry_id) => entry_id,
                    Err(_) => {
                        self.census.open_errors += 1;
                        continue;
                    }
                };
                match store.open_folder(&entry_id) {
                    Ok(child) => folders.push(self.folder(store, child.as_ref(), depth + 1)),
                    Err(_) => self.census.open_errors += 1,
                }
            }
        }

        SourceFolder {
            id: u64::from(nid),
            name,
            folders,
            messages,
        }
    }

    fn message(&mut self, message: &dyn PstMessage, nid: u64) -> SourceMessage {
        let properties = message.properties();

        let subject = properties
            .get(PROP_SUBJECT)
            .and_then(property_text)
            .map(|raw| {
                let (text, marker) = strip_subject_marker(&raw);
                if marker {
                    self.census.subject_markers_stripped += 1;
                }
                text.to_string()
            });
        let internet_id = properties
            .get(PROP_INTERNET_MESSAGE_ID)
            .and_then(property_text);
        self.ids.note(internet_id.as_deref(), &mut self.census);
        if let Ok(class) = properties.message_class() {
            let is_note = class
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("IPM.Note"));
            if !is_note {
                self.census.non_mail_items += 1;
            }
        }

        SourceMessage {
            id: nid,
            subject,
            time: message_time(message),
            attachments: self.attachments(message),
        }
    }

    fn attachments(&mut self, message: &dyn PstMessage) -> Vec<SourceAttachment> {
        let Some(table_rc) = message.attachment_table() else {
            return Vec::new();
        };
        let table: &dyn TableContext = table_rc.as_ref();
        let context = table.context();
        let long_idx = column_index(context, PROP_ATTACH_LONG_FILENAME);
        let short_idx = column_index(context, PROP_ATTACH_FILENAME);
        let method_idx = column_index(context, PROP_ATTACH_METHOD);
        if long_idx.is_some() {
            self.census.attachment_tables_with_long_name_column += 1;
        }

        let mut attachments = Vec::new();
        for row in table.rows_matrix() {
            self.next_attachment_id += 1;
            let id = self.next_attachment_id;
            let Ok(values) = row.columns(context) else {
                self.census.attachment_row_read_errors += 1;
                attachments.push(SourceAttachment {
                    id,
                    name: None,
                    embedded: None,
                });
                continue;
            };
            let name = long_idx
                .and_then(|idx| read_text_at(table, context, &values, idx))
                .or_else(|| short_idx.and_then(|idx| read_text_at(table, context, &values, idx)))
                .filter(|n| !n.trim().is_empty());
            if name.is_none() {
                self.census.attachments_without_name += 1;
            }
            let method = method_idx.and_then(|idx| read_i32_at(table, context, &values, idx));
            if method == Some(ATTACH_METHOD_EMBEDDED_MESSAGE) {
                self.census.embedded_attachments_not_opened += 1;
            }
            attachments.push(SourceAttachment {
                id,
                name,
                embedded: None,
            });
        }
        attachments
    }
}
