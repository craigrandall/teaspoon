//! Attachment-reach spike (M4i step 0): a temporary, read-only, counts-only mode that answers
//! one question before any PST export code is written. Can the concrete `outlook-pst` route
//! (`UnicodeMessage::read`, `UnicodeAttachment::read`, `data()`) deliver every attachment's
//! bytes from a Unicode PST, correctly, and at what memory cost?
//!
//! It reads each attachment's payload into memory, hashes it, and drops it. Nothing is staged or
//! written to the archive. Standard output carries counts only (no names, subjects, or paths).
//! With `--spike-hash-file FILE` it also writes one line per attachment row (ordinals, method,
//! kind, size, SHA-256; no names) to FILE, for comparison with Outlook's Save As hashes. That
//! file is for the small tester PST only: it is refused for a PST over 256 MiB and for an
//! existing FILE.
//!
//! The spike is deleted or folded into a gate once the exporter replaces it. The reading itself
//! lives in `pst_attachments.rs` and is meant to be reused.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::messaging::attachment::{
    Attachment as PstAttachment, AttachmentData, UnicodeAttachment,
};
use outlook_pst::messaging::folder::Folder as PstFolder;
use outlook_pst::messaging::message::{Message as PstMessage, UnicodeMessage};
use outlook_pst::messaging::store::{Store, UnicodeStore};
use outlook_pst::ndb::node_id::NodeId;
use sha2::{Digest, Sha256};

use crate::pst_attachments::{
    AttachmentRow, attachment_row_count, attachment_rows, error_label, message_nodes, method_label,
    open_attachment, open_unicode_pst, subfolder_nodes,
};
use crate::pst_password::{enforce, supplied_from_env};
use crate::pst_text::TextReader;
use crate::shared::{MAX_FOLDER_DEPTH, PROP_BODY};
use crate::stage::hex;

/// The per-attachment hash file is refused for a PST larger than this, so that a list of every
/// attachment of a large mailbox cannot be made by accident.
const HASH_FILE_MAX_PST_BYTES: u64 = 256 * 1024 * 1024;

/// Folder, message, and attachment ordinals (walk order, 0-based) of one attachment row.
type Ids = (u64, u64, u64);

#[derive(Default)]
struct Counts {
    folders: u64,
    folder_open_errors: u64,
    messages_listed: u64,
    message_open_errors: u64,
    messages_without_attachment_table: u64,

    attachment_rows: u64,
    row_column_read_errors: u64,
    read_ok: u64,
    read_errors: u64,

    binary: u64,
    binary_zero_length: u64,
    binary_bytes_total: u64,
    binary_bytes_max: u64,
    binary_bytes_min_nonzero: Option<u64>,
    table_size_equals_data_len: u64,
    table_size_differs: u64,
    table_size_unknown: u64,

    embedded: u64,
    embedded_with_attachment_table: u64,
    embedded_attachment_rows: u64,
    embedded_class_unreadable: u64,

    method_row_vs_object_mismatch: u64,
    size_row_vs_object_mismatch: u64,

    /// `method=<m> kind=<binary|message|none>` to the number of attachments read.
    data_by_method_kind: BTreeMap<String, u64>,
    /// `method=<m> error=<variant> table_size=<zero|nonzero|unknown>` to the number of failures.
    errors_by_method_variant: BTreeMap<String, u64>,
}

struct Spike<'a> {
    store: &'a Rc<UnicodeStore>,
    /// Reads the attachment table's text columns; the spike does not report them.
    text: TextReader,
    counts: Counts,
    hash_file: Option<BufWriter<File>>,
    hash_lines: u64,
    next_folder_ix: u64,
}

/// Runs the spike on a Unicode PST. `hash_file`, when given, is created new (an existing file is
/// refused) and receives one line per attachment row.
pub(crate) fn run_spike_attachments(path: &Path, hash_file: Option<&Path>) -> Result<()> {
    if hash_file.is_some() {
        let size = std::fs::metadata(path)
            .context("failed to read the PST's size")?
            .len();
        if size > HASH_FILE_MAX_PST_BYTES {
            anyhow::bail!(
                "--spike-hash-file is refused for a PST larger than 256 MiB: the per-attachment list is for the small tester only"
            );
        }
    }

    let pst = open_unicode_pst(path)?;
    let store = UnicodeStore::read(Rc::new(pst)).context("failed to read the PST store")?;

    // Same rule as the diagnostic and the planner: a protected PST is refused unless the
    // password in TSP_PST_PASSWORD verifies. A refusal is an early return before anything is
    // printed.
    let outcome = enforce(&*store, supplied_from_env()?)?;

    let hash_writer = match hash_file {
        Some(target) => Some(create_hash_file(target)?),
        None => None,
    };

    let ipm_id = store
        .properties()
        .ipm_sub_tree_entry_id()
        .context("PST has no IPM subtree entry ID")?;
    let ipm = store
        .open_folder(&ipm_id)
        .context("failed to open IPM subtree")?;

    let mut spike = Spike {
        store: &store,
        text: TextReader::new(),
        counts: Counts::default(),
        hash_file: hash_writer,
        hash_lines: 0,
        next_folder_ix: 0,
    };
    spike.walk_folder(ipm.as_ref(), 0)?;

    if let Some(file) = spike.hash_file.as_mut() {
        file.flush().context("failed to write the hash file")?;
    }
    let hash_lines = spike.hash_file.is_some().then_some(spike.hash_lines);
    for line in report_lines(&spike.counts, outcome.key(), hash_lines) {
        println!("{line}");
    }
    Ok(())
}

/// Creates the hash file (it must not exist) and writes its header line.
fn create_hash_file(target: &Path) -> Result<BufWriter<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .context("failed to create the hash file (it must not already exist)")?;
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        "# folder\tmessage\tattachment\tmethod\tkind\tbytes\tsha256\textra"
    )
    .context("failed to write the hash file")?;
    Ok(writer)
}

impl Spike<'_> {
    /// Walks `folder` and everything below it in table order. Below [`MAX_FOLDER_DEPTH`] the
    /// subfolders are not opened (the same cap as the diagnostic); each is counted in
    /// `folder_open_errors`.
    fn walk_folder(&mut self, folder: &dyn PstFolder, depth: usize) -> Result<()> {
        let folder_ix = self.next_folder_ix;
        self.next_folder_ix += 1;
        self.counts.folders += 1;

        for (message_ix, node) in message_nodes(folder).into_iter().enumerate() {
            self.counts.messages_listed += 1;
            self.read_message(node, folder_ix, message_ix as u64)?;
        }

        for node in subfolder_nodes(folder) {
            if depth >= MAX_FOLDER_DEPTH {
                self.counts.folder_open_errors += 1;
                continue;
            }
            let Ok(entry_id) = self.store.properties().make_entry_id(node) else {
                self.counts.folder_open_errors += 1;
                continue;
            };
            match self.store.open_folder(&entry_id) {
                Ok(child) => self.walk_folder(child.as_ref(), depth + 1)?,
                Err(_) => self.counts.folder_open_errors += 1,
            }
        }
        Ok(())
    }

    fn read_message(&mut self, node: NodeId, folder_ix: u64, message_ix: u64) -> Result<()> {
        let Ok(entry_id) = self.store.properties().make_entry_id(node) else {
            self.counts.message_open_errors += 1;
            return Ok(());
        };
        let Ok(message) = UnicodeMessage::read(Rc::clone(self.store), &entry_id, None) else {
            self.counts.message_open_errors += 1;
            return Ok(());
        };
        self.read_attachments(&message, folder_ix, message_ix)
    }

    fn read_attachments(
        &mut self,
        message: &Rc<UnicodeMessage>,
        folder_ix: u64,
        message_ix: u64,
    ) -> Result<()> {
        let Some(rows) = attachment_rows(&**message, &mut self.text) else {
            self.counts.messages_without_attachment_table += 1;
            return Ok(());
        };
        for (attachment_ix, row) in rows.into_iter().enumerate() {
            self.counts.attachment_rows += 1;
            if row.columns_unreadable {
                self.counts.row_column_read_errors += 1;
            }
            let ids: Ids = (folder_ix, message_ix, attachment_ix as u64);
            match open_attachment(message, row.node) {
                Ok(attachment) => self.record_attachment(&attachment, &row, ids)?,
                Err(err) => self.record_error(&err, &row, ids)?,
            }
        }
        Ok(())
    }

    fn record_attachment(
        &mut self,
        attachment: &UnicodeAttachment,
        row: &AttachmentRow,
        ids: Ids,
    ) -> Result<()> {
        self.counts.read_ok += 1;

        // The row ID is taken as the attachment's sub-node ID. If that were wrong, the object
        // read would be a different attachment, and its own method and size would disagree with
        // the row's. Count the disagreements.
        let properties = attachment.properties();
        let object_method = properties.attachment_method().ok();
        let object_size = properties.attachment_size().ok();
        if object_method.is_some() && row.method.is_some() && object_method != row.method {
            self.counts.method_row_vs_object_mismatch += 1;
        }
        if object_size.is_some() && row.size.is_some() && object_size != row.size {
            self.counts.size_row_vs_object_mismatch += 1;
        }

        let method = method_label(object_method.or(row.method));
        match attachment.data() {
            Some(AttachmentData::Binary(binary)) => {
                let bytes = binary.buffer();
                let len = bytes.len() as u64;
                self.note_binary(row, len);
                self.count_data(method, "binary");
                let digest = hex(Sha256::digest(bytes));
                self.write_line(ids, method, "binary", Some(len), Some(&digest), "-")
            }
            Some(AttachmentData::Message(embedded)) => {
                let embedded: &dyn PstMessage = embedded.as_ref();
                self.counts.embedded += 1;
                let nested_rows = attachment_row_count(embedded);
                if let Some(rows) = nested_rows {
                    self.counts.embedded_with_attachment_table += 1;
                    self.counts.embedded_attachment_rows += rows;
                }
                if embedded.properties().message_class().is_err() {
                    self.counts.embedded_class_unreadable += 1;
                }
                let message_size = embedded
                    .properties()
                    .message_size()
                    .ok()
                    .map_or_else(|| "-".to_string(), |size| size.to_string());
                let body = match embedded.properties().get(PROP_BODY) {
                    Some(PropertyValue::Unicode(value)) => format!("{}u16", value.buffer().len()),
                    Some(PropertyValue::String8(value)) => format!("{}b", value.buffer().len()),
                    _ => "-".to_string(),
                };
                let nested = nested_rows.unwrap_or(0);
                let extra =
                    format!("message_size={message_size};body_plain={body};nested_rows={nested}");
                self.count_data(method, "message");
                self.write_line(ids, method, "message", None, None, &extra)
            }
            None => {
                self.count_data(method, "none");
                self.write_line(ids, method, "none", None, None, "-")
            }
        }
    }

    fn note_binary(&mut self, row: &AttachmentRow, len: u64) {
        let counts = &mut self.counts;
        counts.binary += 1;
        counts.binary_bytes_total += len;
        counts.binary_bytes_max = counts.binary_bytes_max.max(len);
        if len == 0 {
            counts.binary_zero_length += 1;
        } else {
            counts.binary_bytes_min_nonzero = Some(
                counts
                    .binary_bytes_min_nonzero
                    .map_or(len, |smallest| smallest.min(len)),
            );
        }
        // Informational only: PidTagAttachSize is not expected to equal the data length.
        match row.size.and_then(|size| u64::try_from(size).ok()) {
            Some(size) if size == len => counts.table_size_equals_data_len += 1,
            Some(_) => counts.table_size_differs += 1,
            None => counts.table_size_unknown += 1,
        }
    }

    fn count_data(&mut self, method: &str, kind: &str) {
        *self
            .counts
            .data_by_method_kind
            .entry(format!("method={method} kind={kind}"))
            .or_insert(0) += 1;
    }

    fn record_error(&mut self, err: &io::Error, row: &AttachmentRow, ids: Ids) -> Result<()> {
        self.counts.read_errors += 1;
        let label = error_label(err);
        let method = method_label(row.method);
        let table_size = match row.size {
            Some(0) => "zero",
            Some(_) => "nonzero",
            None => "unknown",
        };
        *self
            .counts
            .errors_by_method_variant
            .entry(format!(
                "method={method} error={label} table_size={table_size}"
            ))
            .or_insert(0) += 1;
        self.write_line(ids, method, "error", None, None, &label)
    }

    /// Writes one line to the hash file, if there is one.
    fn write_line(
        &mut self,
        ids: Ids,
        method: &str,
        kind: &str,
        bytes: Option<u64>,
        sha256: Option<&str>,
        extra: &str,
    ) -> Result<()> {
        let Some(file) = self.hash_file.as_mut() else {
            return Ok(());
        };
        let bytes = bytes.map_or_else(|| "-".to_string(), |len| len.to_string());
        let sha256 = sha256.unwrap_or("-");
        writeln!(
            file,
            "{}\t{}\t{}\t{method}\t{kind}\t{bytes}\t{sha256}\t{extra}",
            ids.0, ids.1, ids.2
        )
        .context("failed to write the hash file")?;
        self.hash_lines += 1;
        Ok(())
    }
}

/// The report, one `key=value` (or `key a=b count=n`) line each. Counts only.
fn report_lines(counts: &Counts, password_key: &str, hash_lines: Option<u64>) -> Vec<String> {
    let min_nonzero = counts
        .binary_bytes_min_nonzero
        .map_or_else(|| "none".to_string(), |len| len.to_string());
    let rows_unaccounted = counts
        .attachment_rows
        .saturating_sub(counts.read_ok + counts.read_errors);

    let mut lines = vec![
        "spike=pst_attachments".to_string(),
        "pst_format=unicode".to_string(),
        format!("pst_password={password_key}"),
        format!("spike_folders={}", counts.folders),
        format!("spike_folder_open_errors={}", counts.folder_open_errors),
        format!("spike_messages_listed={}", counts.messages_listed),
        format!("spike_message_open_errors={}", counts.message_open_errors),
        format!(
            "spike_messages_without_attachment_table={}",
            counts.messages_without_attachment_table
        ),
        format!("spike_attachment_rows={}", counts.attachment_rows),
        format!(
            "spike_row_column_read_errors={}",
            counts.row_column_read_errors
        ),
        format!("spike_read_ok={}", counts.read_ok),
        format!("spike_read_errors={}", counts.read_errors),
        format!("spike_rows_unaccounted={rows_unaccounted}"),
    ];
    for (key, count) in &counts.data_by_method_kind {
        lines.push(format!("spike_data {key} count={count}"));
    }
    for (key, count) in &counts.errors_by_method_variant {
        lines.push(format!("spike_read_error {key} count={count}"));
    }
    lines.extend([
        format!("spike_binary={}", counts.binary),
        format!("spike_binary_zero_length={}", counts.binary_zero_length),
        format!("spike_binary_bytes_total={}", counts.binary_bytes_total),
        format!("spike_binary_bytes_max={}", counts.binary_bytes_max),
        format!("spike_binary_bytes_min_nonzero={min_nonzero}"),
        format!(
            "spike_table_size_equals_data_len={}",
            counts.table_size_equals_data_len
        ),
        format!("spike_table_size_differs={}", counts.table_size_differs),
        format!("spike_table_size_unknown={}", counts.table_size_unknown),
        format!("spike_embedded_messages={}", counts.embedded),
        format!(
            "spike_embedded_with_attachment_table={}",
            counts.embedded_with_attachment_table
        ),
        format!(
            "spike_embedded_attachment_rows_unreachable={}",
            counts.embedded_attachment_rows
        ),
        format!(
            "spike_embedded_class_unreadable={}",
            counts.embedded_class_unreadable
        ),
        format!(
            "spike_method_row_vs_object_mismatch={}",
            counts.method_row_vs_object_mismatch
        ),
        format!(
            "spike_size_row_vs_object_mismatch={}",
            counts.size_row_vs_object_mismatch
        ),
    ]);
    if let Some(lines_written) = hash_lines {
        lines.push(format!("spike_hash_file_lines={lines_written}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_run_reports_no_unaccounted_rows_and_no_smallest_payload() {
        let lines = report_lines(&Counts::default(), "none", None);
        assert!(lines.contains(&"spike_rows_unaccounted=0".to_string()));
        assert!(lines.contains(&"spike_binary_bytes_min_nonzero=none".to_string()));
        assert!(lines.contains(&"pst_password=none".to_string()));
        assert!(!lines.iter().any(|l| l.starts_with("spike_hash_file_lines")));
    }

    #[test]
    fn rows_that_are_neither_read_nor_failed_are_counted_as_unaccounted() {
        let counts = Counts {
            attachment_rows: 4,
            read_ok: 2,
            read_errors: 1,
            ..Counts::default()
        };
        let lines = report_lines(&counts, "verified", Some(4));
        assert!(lines.contains(&"spike_rows_unaccounted=1".to_string()));
        assert!(lines.contains(&"spike_hash_file_lines=4".to_string()));
    }

    #[test]
    fn the_cross_tabs_print_one_sorted_line_per_key() {
        let mut counts = Counts::default();
        counts
            .data_by_method_kind
            .insert("method=ole kind=binary".to_string(), 2);
        counts
            .data_by_method_kind
            .insert("method=by_value kind=binary".to_string(), 79);
        counts.errors_by_method_variant.insert(
            "method=by_value error=InvalidMessageObjectData table_size=zero".to_string(),
            3,
        );
        let lines = report_lines(&counts, "none", None);
        let data: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|line| line.starts_with("spike_data "))
            .collect();
        assert_eq!(
            data,
            [
                "spike_data method=by_value kind=binary count=79",
                "spike_data method=ole kind=binary count=2"
            ]
        );
        let expected =
            "spike_read_error method=by_value error=InvalidMessageObjectData table_size=zero count=3"
                .to_string();
        assert!(lines.contains(&expected));
    }

    #[test]
    fn no_report_line_can_carry_a_path_or_a_name() {
        let lines = report_lines(&Counts::default(), "none", None);
        for line in &lines {
            assert!(
                line.chars()
                    .all(|c| c.is_ascii_alphanumeric() || " =_".contains(c)),
                "unexpected character in a report line"
            );
        }
    }
}
