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
//! Attachment-name study (M4g): the same run also compares, for every attachment row, the name
//! the planner takes from the attachment table (the long-name column, else the short one) with
//! the long, short, and display names stored on the attachment object itself, and counts how
//! they relate (`spike_name_*` lines; names are never printed). `--spike-names-file FILE`, for
//! the tester only, additionally writes the names themselves, one line per row, to FILE. The
//! run also times opening each attachment object, split by whether the planner's name looks
//! like an 8.3 alias, to price reading names from the objects at planning time.
//!
//! The spike is deleted or folded into a gate once the exporter replaces it. The reading itself
//! lives in `pst_attachments.rs` and is meant to be reused.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Context, Result};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::ltp::table_context::TableContext;
use outlook_pst::messaging::attachment::{
    Attachment as PstAttachment, AttachmentData, UnicodeAttachment,
};
use outlook_pst::messaging::folder::Folder as PstFolder;
use outlook_pst::messaging::message::{Message as PstMessage, UnicodeMessage};
use outlook_pst::messaging::store::{Store, UnicodeStore};
use outlook_pst::ndb::node_id::NodeId;
use sha2::{Digest, Sha256};

use crate::dry_run::{PROP_ATTACH_FILENAME, PROP_ATTACH_LONG_FILENAME};
use crate::oxmsg_decode::{PROP_INTERNET_CODEPAGE, PROP_MESSAGE_CODEPAGE};
use crate::pst::column_index;
use crate::pst_attachments::{
    AttachmentRow, attachment_row_count, attachment_rows, error_label, message_nodes, method_label,
    open_attachment, open_unicode_pst, subfolder_nodes,
};
use crate::pst_password::{enforce, supplied_from_env};
use crate::pst_text::{TextReader, integer32, read_text_at};
use crate::shared::{MAX_FOLDER_DEPTH, PROP_BODY};
use crate::stage::hex;

/// The per-attachment hash file is refused for a PST larger than this, so that a list of every
/// attachment of a large mailbox cannot be made by accident.
const HASH_FILE_MAX_PST_BYTES: u64 = 256 * 1024 * 1024;

/// Folder, message, and attachment ordinals (walk order, 0-based) of one attachment row.
type Ids = (u64, u64, u64);

/// PidTagDisplayName of an attachment object.
const PROP_ATTACH_DISPLAY_NAME: u16 = 0x3001;

/// Counts for the attachment-name study: how the name the planner takes from the attachment
/// table compares with the names stored on the attachment object. Counts only.
#[derive(Default)]
struct NameCounts {
    tables: u64,
    tables_long_column: u64,
    rows_in_long_tables: u64,
    /// Rows whose name columns could not be lined up with the attachment rows (never expected).
    rows_misaligned: u64,
    rows: u64,
    table_long_present: u64,
    table_short_present: u64,
    planner_none: u64,
    planner_alias: u64,
    planner_other: u64,
    /// Rows whose attachment object was read; the next fields count over these.
    object_rows: u64,
    object_long_present: u64,
    object_long_is_alias: u64,
    object_short_present: u64,
    object_display_present: u64,
    display_equals_long: u64,
    display_differs_from_long: u64,
    alias_equals_object_short: u64,
    alias_ext_equal: u64,
    alias_ext_differs: u64,
    /// Opening the attachment object, split by the planner's name (an 8.3 alias, or anything
    /// else): rows, milliseconds, and payload bytes.
    open_rows_alias: u64,
    open_ms_alias: u64,
    open_bytes_alias: u64,
    open_rows_other: u64,
    open_ms_other: u64,
    open_bytes_other: u64,
    /// Lines written to the names file, when there is one.
    file_lines: Option<u64>,
    /// `planner=<none|alias|other> object_long=<...>` (or `object=unreadable method=<m>`) to the
    /// number of rows.
    cross: BTreeMap<String, u64>,
}

/// The attachment table's name columns for one row, read the way the planner reads them.
struct NameColumns {
    long: Option<String>,
    short: Option<String>,
    /// The planner's name: the long-name column's value when it has one, else the short-name
    /// column's; blank is no name.
    planner: Option<String>,
}

/// The name columns of every row of `message`'s attachment table, in table order, and whether
/// the table has a long-name column at all; `None` when the message has no attachment table.
fn name_columns(
    message: &dyn PstMessage,
    text: &mut TextReader,
) -> Option<(bool, Vec<NameColumns>)> {
    let table_rc = message.attachment_table()?;
    let table: &dyn TableContext = table_rc.as_ref();
    let context = table.context();
    let long_idx = column_index(context, PROP_ATTACH_LONG_FILENAME);
    let short_idx = column_index(context, PROP_ATTACH_FILENAME);
    let columns: Vec<NameColumns> = table
        .rows_matrix()
        .map(|row| {
            let (long, short) = match row.columns(context) {
                Ok(values) => (
                    long_idx.and_then(|idx| read_text_at(table, context, &values, idx, text)),
                    short_idx.and_then(|idx| read_text_at(table, context, &values, idx, text)),
                ),
                Err(_) => (None, None),
            };
            let planner = long
                .clone()
                .or_else(|| short.clone())
                .filter(|name| !name.trim().is_empty());
            NameColumns {
                long,
                short,
                planner,
            }
        })
        .collect();
    Some((long_idx.is_some(), columns))
}

/// True for a name shaped like a Windows 8.3 short alias: one to six characters, `~`, one to
/// three digits, and an optional extension of at most three characters (`SCREEN~1.JPG`).
fn looks_like_short_alias(name: &str) -> bool {
    let (stem, extension_ok) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension.chars().count() <= 3),
        None => (name, true),
    };
    if !extension_ok {
        return false;
    }
    let Some((head, digits)) = stem.rsplit_once('~') else {
        return false;
    };
    (1..=6).contains(&head.chars().count())
        && !head
            .chars()
            .any(|c| matches!(c, '.' | '~' | '\\' | '/' | ':'))
        && (1..=3).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The text after the last dot of `name`, or nothing when it has no dot.
fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

/// A name for the names file: `-` when there is none, else the text with its escapes shown.
fn show(value: Option<&str>) -> String {
    value.map_or_else(|| "-".to_string(), |text| format!("{text:?}"))
}

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

    /// The attachment-name study (M4g, names).
    names: NameCounts,
}

struct Spike<'a> {
    store: &'a Rc<UnicodeStore>,
    /// Reads the attachment table's text columns; the spike does not report them.
    text: TextReader,
    counts: Counts,
    hash_file: Option<BufWriter<File>>,
    hash_lines: u64,
    names_file: Option<BufWriter<File>>,
    next_folder_ix: u64,
}

/// Runs the spike on a Unicode PST. `hash_file` and `names_file`, when given, are created new
/// (an existing file is refused) and receive one line per attachment row.
pub(crate) fn run_spike_attachments(
    path: &Path,
    hash_file: Option<&Path>,
    names_file: Option<&Path>,
) -> Result<()> {
    if hash_file.is_some() || names_file.is_some() {
        let size = std::fs::metadata(path)
            .context("failed to read the PST's size")?
            .len();
        if size > HASH_FILE_MAX_PST_BYTES {
            anyhow::bail!(
                "--spike-hash-file and --spike-names-file are refused for a PST larger than 256 MiB: the per-attachment lists are for the small tester only"
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
    let names_writer = match names_file {
        Some(target) => Some(create_names_file(target)?),
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
        names_file: names_writer,
        next_folder_ix: 0,
    };
    if names_file.is_some() {
        spike.counts.names.file_lines = Some(0);
    }
    spike.walk_folder(ipm.as_ref(), 0)?;

    if let Some(file) = spike.hash_file.as_mut() {
        file.flush().context("failed to write the hash file")?;
    }
    let hash_lines = spike.hash_file.is_some().then_some(spike.hash_lines);
    if let Some(file) = spike.names_file.as_mut() {
        file.flush().context("failed to write the names file")?;
    }
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

/// Creates the names file (it must not exist) and writes its header line.
fn create_names_file(target: &Path) -> Result<BufWriter<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .context("failed to create the names file (it must not already exist)")?;
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        "# folder\tmessage\tattachment\tmethod\tclass\tplanner\ttable_long\ttable_short\tobject_long\tobject_short\tobject_display"
    )
    .context("failed to write the names file")?;
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
        // Strings are decoded under this message's code page, as the planner does.
        let properties = message.properties();
        self.text.use_message_codepage(
            integer32(properties.get(PROP_MESSAGE_CODEPAGE)),
            integer32(properties.get(PROP_INTERNET_CODEPAGE)),
        );
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
        // The name columns come from the same table, in the same row order.
        let (long_column, columns) = name_columns(&**message, &mut self.text).unwrap_or_default();
        let aligned = columns.len() == rows.len();
        if aligned {
            let names = &mut self.counts.names;
            names.tables += 1;
            if long_column {
                names.tables_long_column += 1;
                names.rows_in_long_tables += rows.len() as u64;
            }
        } else {
            self.counts.names.rows_misaligned += rows.len() as u64;
        }
        for (attachment_ix, row) in rows.into_iter().enumerate() {
            self.counts.attachment_rows += 1;
            if row.columns_unreadable {
                self.counts.row_column_read_errors += 1;
            }
            let ids: Ids = (folder_ix, message_ix, attachment_ix as u64);
            let name = if aligned {
                columns.get(attachment_ix)
            } else {
                None
            };
            let started = Instant::now();
            let opened = open_attachment(message, row.node);
            let open_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            match opened {
                Ok(attachment) => self.record_attachment(&attachment, &row, name, open_ms, ids)?,
                Err(err) => self.record_error(&err, &row, name, open_ms, ids)?,
            }
        }
        Ok(())
    }

    fn record_attachment(
        &mut self,
        attachment: &UnicodeAttachment,
        row: &AttachmentRow,
        name: Option<&NameColumns>,
        open_ms: u64,
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
        let payload_len = match attachment.data() {
            Some(AttachmentData::Binary(binary)) => binary.buffer().len() as u64,
            _ => 0,
        };
        self.note_names(name, Some(attachment), method, open_ms, payload_len, ids)?;
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

    /// Counts how the attachment table's name for this row compares with the names stored on
    /// the attachment object, and writes the names-file line, if there is one. `object` is
    /// `None` when the object could not be read. `open_ms` and `payload_len` are the time spent
    /// opening the object and the payload it carried, for the cost question.
    fn note_names(
        &mut self,
        name: Option<&NameColumns>,
        object: Option<&UnicodeAttachment>,
        method: &str,
        open_ms: u64,
        payload_len: u64,
        ids: Ids,
    ) -> Result<()> {
        let Some(name) = name else {
            return Ok(());
        };
        let planner = name.planner.as_deref();
        let planner_class = match planner {
            None => "none",
            Some(text) if looks_like_short_alias(text) => "alias",
            Some(_) => "other",
        };
        let (object_long, object_short, object_display) = match object {
            Some(attachment) => {
                let properties = attachment.properties();
                let mut read = |id: u16| {
                    properties
                        .get(id)
                        .and_then(|value| self.text.text(value))
                        .filter(|text| !text.trim().is_empty())
                };
                (
                    read(PROP_ATTACH_LONG_FILENAME),
                    read(PROP_ATTACH_FILENAME),
                    read(PROP_ATTACH_DISPLAY_NAME),
                )
            }
            None => (None, None, None),
        };
        let long = object_long.as_deref();
        let short = object_short.as_deref();
        let display = object_display.as_deref();
        let object_class = if object.is_none() {
            format!("object=unreadable method={method}")
        } else {
            let label = match (planner, long) {
                (_, None) => "absent",
                (None, Some(_)) => "present",
                (Some(p), Some(o)) if p == o => "same",
                (Some(p), Some(o)) if p.eq_ignore_ascii_case(o) => "case_only",
                (Some(_), Some(o)) if looks_like_short_alias(o) => "alias",
                (Some(_), Some(_)) => "differs",
            };
            format!("object_long={label}")
        };
        let class = format!("planner={planner_class} {object_class}");

        let alias_to_long = match (planner_class, planner, long) {
            ("alias", Some(p), Some(o)) if !looks_like_short_alias(o) && p != o => {
                Some(extension_of(p).eq_ignore_ascii_case(extension_of(o)))
            }
            _ => None,
        };
        let display_vs_long = match (display, long) {
            (Some(d), Some(l)) => Some(d == l),
            _ => None,
        };
        let counts = &mut self.counts.names;
        counts.rows += 1;
        if name
            .long
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
        {
            counts.table_long_present += 1;
        }
        if name
            .short
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
        {
            counts.table_short_present += 1;
        }
        match planner_class {
            "none" => counts.planner_none += 1,
            "alias" => counts.planner_alias += 1,
            _ => counts.planner_other += 1,
        }
        if object.is_some() {
            counts.object_rows += 1;
            counts.object_long_present += u64::from(long.is_some());
            counts.object_long_is_alias += u64::from(long.is_some_and(looks_like_short_alias));
            counts.object_short_present += u64::from(short.is_some());
            counts.object_display_present += u64::from(display.is_some());
            counts.alias_equals_object_short +=
                u64::from(planner_class == "alias" && short == planner);
            match display_vs_long {
                Some(true) => counts.display_equals_long += 1,
                Some(false) => counts.display_differs_from_long += 1,
                None => {}
            }
            match alias_to_long {
                Some(true) => counts.alias_ext_equal += 1,
                Some(false) => counts.alias_ext_differs += 1,
                None => {}
            }
        }
        *counts.cross.entry(class.clone()).or_insert(0) += 1;
        if planner_class == "alias" {
            counts.open_rows_alias += 1;
            counts.open_ms_alias += open_ms;
            counts.open_bytes_alias += payload_len;
        } else {
            counts.open_rows_other += 1;
            counts.open_ms_other += open_ms;
            counts.open_bytes_other += payload_len;
        }

        if let Some(file) = self.names_file.as_mut() {
            let (folder, message, attachment) = ids;
            let planner_text = show(planner);
            let table_long = show(name.long.as_deref());
            let table_short = show(name.short.as_deref());
            let object_long_text = show(long);
            let object_short_text = show(short);
            let object_display_text = show(display);
            writeln!(
                file,
                "{folder}\t{message}\t{attachment}\t{method}\t{class}\t{planner_text}\t{table_long}\t{table_short}\t{object_long_text}\t{object_short_text}\t{object_display_text}"
            )
            .context("failed to write the names file")?;
            if let Some(written) = self.counts.names.file_lines.as_mut() {
                *written += 1;
            }
        }
        Ok(())
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

    fn record_error(
        &mut self,
        err: &io::Error,
        row: &AttachmentRow,
        name: Option<&NameColumns>,
        open_ms: u64,
        ids: Ids,
    ) -> Result<()> {
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
        self.note_names(name, None, method, open_ms, 0, ids)?;
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
    let names = &counts.names;
    lines.extend([
        format!("spike_name_tables={}", names.tables),
        format!("spike_name_tables_long_column={}", names.tables_long_column),
        format!(
            "spike_name_rows_in_long_tables={}",
            names.rows_in_long_tables
        ),
        format!("spike_name_rows_misaligned={}", names.rows_misaligned),
        format!("spike_name_rows={}", names.rows),
        format!("spike_name_table_long_present={}", names.table_long_present),
        format!(
            "spike_name_table_short_present={}",
            names.table_short_present
        ),
        format!("spike_name_planner_none={}", names.planner_none),
        format!("spike_name_planner_alias={}", names.planner_alias),
        format!("spike_name_planner_other={}", names.planner_other),
        format!("spike_name_object_rows={}", names.object_rows),
        format!(
            "spike_name_object_long_present={}",
            names.object_long_present
        ),
        format!(
            "spike_name_object_long_is_alias={}",
            names.object_long_is_alias
        ),
        format!(
            "spike_name_object_short_present={}",
            names.object_short_present
        ),
        format!(
            "spike_name_object_display_present={}",
            names.object_display_present
        ),
        format!(
            "spike_name_display_equals_long={}",
            names.display_equals_long
        ),
        format!(
            "spike_name_display_differs_from_long={}",
            names.display_differs_from_long
        ),
        format!(
            "spike_name_alias_equals_object_short={}",
            names.alias_equals_object_short
        ),
        format!("spike_name_alias_ext_equal={}", names.alias_ext_equal),
        format!("spike_name_alias_ext_differs={}", names.alias_ext_differs),
        format!("spike_name_open_rows_alias={}", names.open_rows_alias),
        format!("spike_name_open_ms_alias={}", names.open_ms_alias),
        format!("spike_name_open_bytes_alias={}", names.open_bytes_alias),
        format!("spike_name_open_rows_other={}", names.open_rows_other),
        format!("spike_name_open_ms_other={}", names.open_ms_other),
        format!("spike_name_open_bytes_other={}", names.open_bytes_other),
    ]);
    for (key, count) in &names.cross {
        lines.push(format!("spike_name_cross {key} count={count}"));
    }
    if let Some(lines_written) = names.file_lines {
        lines.push(format!("spike_names_file_lines={lines_written}"));
    }
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

    #[test]
    fn the_short_alias_shape_matches_only_eight_dot_three_aliases() {
        for alias in [
            "SCREEN~1.JPG",
            "Screen~1.jpg",
            "A~1",
            "ABCDEF~12.TXT",
            "IMAGE~1.PN",
            "X~999.",
        ] {
            assert!(looks_like_short_alias(alias), "{alias}");
        }
        for other in [
            "",
            "image001.png",
            "~WRD0001.jpg",
            "ABCDEFG~1.JPG",
            "SCREEN~1.JPEG",
            "SCREEN~.JPG",
            "SCREEN~1234.JPG",
            "a.b~1.txt",
            "Screenshot_20230114-190529_Twitter.jpg",
        ] {
            assert!(!looks_like_short_alias(other), "{other}");
        }
    }

    #[test]
    fn the_extension_is_the_text_after_the_last_dot() {
        assert_eq!(extension_of("a.b.pdf"), "pdf");
        assert_eq!(extension_of("noext"), "");
        assert_eq!(extension_of("trailing."), "");
    }

    #[test]
    fn a_shown_name_has_its_escapes_and_a_missing_one_is_a_dash() {
        assert_eq!(show(None), "-");
        assert_eq!(show(Some("a\tb")), "\"a\\tb\"");
    }

    #[test]
    fn the_name_cross_tab_prints_one_sorted_line_per_key_and_the_names_file_count() {
        let mut names = NameCounts {
            file_lines: Some(7),
            ..NameCounts::default()
        };
        names
            .cross
            .insert("planner=other object_long=same".to_string(), 5);
        names
            .cross
            .insert("planner=alias object_long=differs".to_string(), 3);
        let counts = Counts {
            names,
            ..Counts::default()
        };
        let lines = report_lines(&counts, "none", None);
        let cross: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|line| line.starts_with("spike_name_cross "))
            .collect();
        assert_eq!(
            cross,
            [
                "spike_name_cross planner=alias object_long=differs count=3",
                "spike_name_cross planner=other object_long=same count=5"
            ]
        );
        assert!(lines.contains(&"spike_names_file_lines=7".to_string()));
    }
}
