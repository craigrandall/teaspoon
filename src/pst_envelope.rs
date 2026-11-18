//! The envelope of a PST message (M4i, drop B): sender, sent-representing, To / Cc / Bcc
//! recipients, submit and delivery times, importance, sensitivity, the transport headers, and the
//! conversation fields, read into the model's [`Envelope`] and counted.
//!
//! These are the properties the `.msg` reader takes (`envelope_props.rs` holds their identifiers
//! and the To / Cc / Bcc rule, so each is defined once); only where they are read from differs.
//! The message-level ones come from the message's property context, which
//! `UnicodeMessage::read(.., None)` reads whole. The recipients come from the recipient table,
//! one row per recipient, the way `pst_attachments.rs` reads the attachment table.
//!
//! Nothing is derived or repaired (the model's rule): an address keeps its display name, address
//! type, address, and SMTP address as separate optional fields, and a recipient with none of the
//! four is listed with an empty address. Strings are read through [`TextReader`], so a
//! `PT_STRING8` value is decoded under the message's code page chain and every loss is counted
//! there, not here.
//!
//! Recipient order: recipients are listed in the order of the recipient table's rows. The `.msg`
//! reader lists them in storage order, which is the recipient index. That the table order is
//! the `PidTagRowid` order is not assumed: [`EnvelopeNotes`] counts the messages where it is, is
//! not, and cannot be checked. The M4g freeze waits for those counts and for the Outlook
//! comparison, because recipient order is archive content.
//!
//! A recipient row that cannot be read, or whose type cannot be read, is counted as unlisted (as
//! the `.msg` reader counts a storage whose type is unreadable), and the export goes on. Every
//! row is accounted for as listed or unlisted; the report's `recipient_rows_unaccounted` must be
//! zero.
//!
//! Containment: this runs inside `PstSource::read_message`, which runs inside `run_contained`,
//! so a panic in the crate becomes a counted read error of the message, as for the rest of it.
//! Nothing here prints; every count is content-free.

use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::ltp::table_context::{TableContext, TableContextInfo};
use outlook_pst::messaging::message::{Message as PstMessage, MessageProperties};

use crate::dry_run::{PROP_DELIVERY_TIME, PROP_SUBMIT_TIME};
use crate::envelope_props::{
    AddressProps, PROP_CONVERSATION_INDEX, PROP_CONVERSATION_TOPIC, PROP_IMPORTANCE,
    PROP_RECIPIENT_ROW_ID, PROP_SENSITIVITY, PROP_TRANSPORT_HEADERS, RECIPIENT_PROPS, SENDER_PROPS,
    SENT_REPRESENTING_PROPS, is_exchange_without_smtp, is_originator_row, recipient_kind,
};
use crate::model::{Address, Envelope, Recipient, RecipientKind};
use crate::pst::{column_index, read_i32_at};
use crate::pst_text::{TextReader, integer32, read_text_at};
use crate::shared::PROP_RECIPIENT_TYPE;

/// Content-free counts gathered while reading the envelopes of the messages of an export. Every
/// field is printed by [`EnvelopeNotes::report_lines`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EnvelopeNotes {
    /// Messages whose envelope was read.
    pub(crate) messages: u64,
    pub(crate) sender_present: u64,
    pub(crate) sent_representing_present: u64,
    pub(crate) submit_time_present: u64,
    pub(crate) delivery_time_present: u64,
    pub(crate) importance_present: u64,
    pub(crate) sensitivity_present: u64,
    pub(crate) conversation_topic_present: u64,
    pub(crate) conversation_index_present: u64,
    pub(crate) transport_headers_present: u64,
    /// Addresses (the sender and the listed recipients) of type `EX` with no SMTP address.
    pub(crate) exchange_without_smtp: u64,
    /// Messages with no recipient table at all.
    pub(crate) recipient_tables_absent: u64,
    /// Recipient tables lacking a column, one count per column the reader uses.
    pub(crate) recipient_tables_without_type_column: u64,
    pub(crate) recipient_tables_without_display_name_column: u64,
    pub(crate) recipient_tables_without_address_type_column: u64,
    pub(crate) recipient_tables_without_email_address_column: u64,
    pub(crate) recipient_tables_without_smtp_address_column: u64,
    pub(crate) recipient_tables_without_row_id_column: u64,
    /// Recipient-table rows seen, and those whose columns could not be read at all.
    pub(crate) recipient_rows: u64,
    pub(crate) recipient_rows_unreadable: u64,
    pub(crate) recipients_to: u64,
    pub(crate) recipients_cc: u64,
    pub(crate) recipients_bcc: u64,
    /// Rows that are not listed: the originator row, a type that is not To / Cc / Bcc, and a row
    /// whose type could not be read (which includes every unreadable row).
    pub(crate) recipients_unlisted_originator: u64,
    pub(crate) recipients_unlisted_other_type: u64,
    pub(crate) recipients_unlisted_type_unreadable: u64,
    /// Messages with two or more recipient rows, by whether the table order is the `PidTagRowid`
    /// order, is not, or could not be checked (no such column, or a row without a value).
    pub(crate) recipient_order_in_row_id_order: u64,
    pub(crate) recipient_order_differs_from_row_id: u64,
    pub(crate) recipient_order_unverifiable: u64,
}

impl EnvelopeNotes {
    /// Every count as `(key, value)`, in print order. The export prints each as
    /// `export_pst_envelope_<key>=<value>`. The derived keys are `sender_missing`,
    /// `recipients_listed`, `recipients_unlisted`, and `recipient_rows_unaccounted` (rows minus
    /// listed minus unlisted, as an absolute difference: it must be zero).
    pub(crate) fn report_lines(&self) -> Vec<(&'static str, u64)> {
        let listed = self.recipients_to + self.recipients_cc + self.recipients_bcc;
        let unlisted = self.recipients_unlisted_originator
            + self.recipients_unlisted_other_type
            + self.recipients_unlisted_type_unreadable;
        vec![
            ("messages", self.messages),
            ("sender_present", self.sender_present),
            (
                "sender_missing",
                self.messages.saturating_sub(self.sender_present),
            ),
            ("sent_representing_present", self.sent_representing_present),
            ("submit_time_present", self.submit_time_present),
            ("delivery_time_present", self.delivery_time_present),
            ("importance_present", self.importance_present),
            ("sensitivity_present", self.sensitivity_present),
            (
                "conversation_topic_present",
                self.conversation_topic_present,
            ),
            (
                "conversation_index_present",
                self.conversation_index_present,
            ),
            ("transport_headers_present", self.transport_headers_present),
            ("exchange_without_smtp", self.exchange_without_smtp),
            ("recipient_tables_absent", self.recipient_tables_absent),
            (
                "recipient_tables_without_type_column",
                self.recipient_tables_without_type_column,
            ),
            (
                "recipient_tables_without_display_name_column",
                self.recipient_tables_without_display_name_column,
            ),
            (
                "recipient_tables_without_address_type_column",
                self.recipient_tables_without_address_type_column,
            ),
            (
                "recipient_tables_without_email_address_column",
                self.recipient_tables_without_email_address_column,
            ),
            (
                "recipient_tables_without_smtp_address_column",
                self.recipient_tables_without_smtp_address_column,
            ),
            (
                "recipient_tables_without_row_id_column",
                self.recipient_tables_without_row_id_column,
            ),
            ("recipient_rows", self.recipient_rows),
            ("recipient_rows_unreadable", self.recipient_rows_unreadable),
            ("recipients_listed", listed),
            ("recipients_to", self.recipients_to),
            ("recipients_cc", self.recipients_cc),
            ("recipients_bcc", self.recipients_bcc),
            ("recipients_unlisted", unlisted),
            (
                "recipients_unlisted_originator",
                self.recipients_unlisted_originator,
            ),
            (
                "recipients_unlisted_other_type",
                self.recipients_unlisted_other_type,
            ),
            (
                "recipients_unlisted_type_unreadable",
                self.recipients_unlisted_type_unreadable,
            ),
            (
                "recipient_order_in_row_id_order",
                self.recipient_order_in_row_id_order,
            ),
            (
                "recipient_order_differs_from_row_id",
                self.recipient_order_differs_from_row_id,
            ),
            (
                "recipient_order_unverifiable",
                self.recipient_order_unverifiable,
            ),
            (
                "recipient_rows_unaccounted",
                self.recipient_rows.abs_diff(listed + unlisted),
            ),
        ]
    }

    /// Counts what a finished envelope carries.
    fn note_envelope(&mut self, envelope: &Envelope) {
        self.messages += 1;
        self.sender_present += u64::from(envelope.sender.is_some());
        self.sent_representing_present += u64::from(envelope.sent_representing.is_some());
        self.submit_time_present += u64::from(envelope.submit_time.is_some());
        self.delivery_time_present += u64::from(envelope.delivery_time.is_some());
        self.importance_present += u64::from(envelope.importance.is_some());
        self.sensitivity_present += u64::from(envelope.sensitivity.is_some());
        self.conversation_topic_present += u64::from(envelope.conversation_topic.is_some());
        self.conversation_index_present += u64::from(envelope.conversation_index.is_some());
        self.transport_headers_present += u64::from(envelope.transport_headers.is_some());
        self.exchange_without_smtp += envelope
            .recipients
            .iter()
            .map(|r| &r.address)
            .chain(envelope.sender.iter())
            .filter(|a| is_exchange_without_smtp(a))
            .count() as u64;
    }

    /// Counts which of the columns the reader uses a recipient table lacks.
    fn note_columns(&mut self, columns: &RecipientColumns) {
        self.recipient_tables_without_type_column += u64::from(columns.recipient_type.is_none());
        self.recipient_tables_without_display_name_column +=
            u64::from(columns.display_name.is_none());
        self.recipient_tables_without_address_type_column +=
            u64::from(columns.address_type.is_none());
        self.recipient_tables_without_email_address_column +=
            u64::from(columns.email_address.is_none());
        self.recipient_tables_without_smtp_address_column +=
            u64::from(columns.smtp_address.is_none());
        self.recipient_tables_without_row_id_column += u64::from(columns.row_id.is_none());
    }

    /// Counts whether a message's recipient rows came in `PidTagRowid` order. Fewer than two
    /// rows have no order to check.
    fn note_order(&mut self, columns: &RecipientColumns, row_ids: &[Option<i32>]) {
        if row_ids.len() < 2 {
            return;
        }
        if columns.row_id.is_none() {
            self.recipient_order_unverifiable += 1;
            return;
        }
        match row_order(row_ids) {
            RowOrder::InOrder => self.recipient_order_in_row_id_order += 1,
            RowOrder::Differs => self.recipient_order_differs_from_row_id += 1,
            RowOrder::Unverifiable => self.recipient_order_unverifiable += 1,
        }
    }
}

/// Whether a message's recipient rows, in table order, are in `PidTagRowid` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowOrder {
    /// No row number is greater than the one after it.
    InOrder,
    Differs,
    /// A row had no readable number.
    Unverifiable,
}

fn row_order(row_ids: &[Option<i32>]) -> RowOrder {
    let mut ids = Vec::with_capacity(row_ids.len());
    for id in row_ids {
        match id {
            Some(value) => ids.push(*value),
            None => return RowOrder::Unverifiable,
        }
    }
    if ids.windows(2).any(|pair| pair[0] > pair[1]) {
        RowOrder::Differs
    } else {
        RowOrder::InOrder
    }
}

/// The positions of the columns the reader uses in one recipient table.
struct RecipientColumns {
    recipient_type: Option<usize>,
    row_id: Option<usize>,
    display_name: Option<usize>,
    address_type: Option<usize>,
    email_address: Option<usize>,
    smtp_address: Option<usize>,
}

impl RecipientColumns {
    fn find(context: &TableContextInfo) -> Self {
        RecipientColumns {
            recipient_type: column_index(context, PROP_RECIPIENT_TYPE),
            row_id: column_index(context, PROP_RECIPIENT_ROW_ID),
            display_name: column_index(context, RECIPIENT_PROPS.name),
            address_type: column_index(context, RECIPIENT_PROPS.address_type),
            email_address: column_index(context, RECIPIENT_PROPS.email_address),
            smtp_address: column_index(context, RECIPIENT_PROPS.smtp_address),
        }
    }
}

/// Reads the envelope of `message`. `text` must already hold the message's code page (the
/// caller sets it once per message); its loss counts are the caller's to collect.
pub(crate) fn read_envelope(
    message: &dyn PstMessage,
    text: &mut TextReader,
    notes: &mut EnvelopeNotes,
) -> Envelope {
    let properties = message.properties();
    let (recipients, recipients_unlisted) = read_recipients(message, text, notes);
    let envelope = Envelope {
        sender: read_address(properties, text, &SENDER_PROPS),
        sent_representing: read_address(properties, text, &SENT_REPRESENTING_PROPS),
        recipients,
        recipients_unlisted,
        submit_time: time_property(properties, PROP_SUBMIT_TIME),
        delivery_time: time_property(properties, PROP_DELIVERY_TIME),
        importance: integer32(properties.get(PROP_IMPORTANCE)),
        sensitivity: integer32(properties.get(PROP_SENSITIVITY)),
        conversation_topic: read_text(properties, text, PROP_CONVERSATION_TOPIC),
        conversation_index: binary_property(properties, PROP_CONVERSATION_INDEX),
        transport_headers: read_text(properties, text, PROP_TRANSPORT_HEADERS),
    };
    notes.note_envelope(&envelope);
    envelope
}

/// A string property of the message, or `None` when absent, empty, or not a string.
fn read_text(properties: &MessageProperties, text: &mut TextReader, id: u16) -> Option<String> {
    properties
        .get(id)
        .and_then(|value| text.text(value))
        .filter(|s| !s.is_empty())
}

/// An address read from the message's properties; `None` when none of the four is present.
fn read_address(
    properties: &MessageProperties,
    text: &mut TextReader,
    props: &AddressProps,
) -> Option<Address> {
    let address = Address {
        display_name: read_text(properties, text, props.name),
        address_type: read_text(properties, text, props.address_type),
        email_address: read_text(properties, text, props.email_address),
        smtp_address: read_text(properties, text, props.smtp_address),
    };
    if address.is_empty() {
        None
    } else {
        Some(address)
    }
}

/// A time property as FILETIME ticks, or `None` when absent or not a time.
fn time_property(properties: &MessageProperties, id: u16) -> Option<i64> {
    match properties.get(id) {
        Some(PropertyValue::Time(ticks)) => Some(*ticks),
        _ => None,
    }
}

/// A binary property's bytes, or `None` when absent, empty, or not binary.
fn binary_property(properties: &MessageProperties, id: u16) -> Option<Vec<u8>> {
    match properties.get(id) {
        Some(PropertyValue::Binary(value)) => {
            Some(value.buffer().to_vec()).filter(|bytes| !bytes.is_empty())
        }
        _ => None,
    }
}

/// To, Cc, and Bcc recipients in table order, plus the number of rows that are none of those.
fn read_recipients(
    message: &dyn PstMessage,
    text: &mut TextReader,
    notes: &mut EnvelopeNotes,
) -> (Vec<Recipient>, usize) {
    let Some(table_rc) = message.recipient_table() else {
        notes.recipient_tables_absent += 1;
        return (Vec::new(), 0);
    };
    let table: &dyn TableContext = table_rc.as_ref();
    let context = table.context();
    let columns = RecipientColumns::find(context);
    notes.note_columns(&columns);

    let mut recipients = Vec::new();
    let mut unlisted = 0usize;
    let mut row_ids: Vec<Option<i32>> = Vec::new();
    for row in table.rows_matrix() {
        notes.recipient_rows += 1;
        let Ok(values) = row.columns(context) else {
            notes.recipient_rows_unreadable += 1;
            notes.recipients_unlisted_type_unreadable += 1;
            unlisted += 1;
            row_ids.push(None);
            continue;
        };
        row_ids.push(
            columns
                .row_id
                .and_then(|index| read_i32_at(table, context, &values, index)),
        );
        let stored = columns
            .recipient_type
            .and_then(|index| read_i32_at(table, context, &values, index));
        let Some(kind) = stored.and_then(recipient_kind) else {
            unlisted += 1;
            match stored {
                None => notes.recipients_unlisted_type_unreadable += 1,
                Some(value) if is_originator_row(value) => {
                    notes.recipients_unlisted_originator += 1;
                }
                Some(_) => notes.recipients_unlisted_other_type += 1,
            }
            continue;
        };
        match kind {
            RecipientKind::To => notes.recipients_to += 1,
            RecipientKind::Cc => notes.recipients_cc += 1,
            RecipientKind::Bcc => notes.recipients_bcc += 1,
        }
        let mut cell = |index: Option<usize>| {
            index
                .and_then(|i| read_text_at(table, context, &values, i, text))
                .filter(|s| !s.is_empty())
        };
        let address = Address {
            display_name: cell(columns.display_name),
            address_type: cell(columns.address_type),
            email_address: cell(columns.email_address),
            smtp_address: cell(columns.smtp_address),
        };
        recipients.push(Recipient { kind, address });
    }
    notes.note_order(&columns, &row_ids);
    (recipients, unlisted)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn columns(row_id: bool) -> RecipientColumns {
        RecipientColumns {
            recipient_type: Some(0),
            row_id: row_id.then_some(1),
            display_name: Some(2),
            address_type: None,
            email_address: Some(4),
            smtp_address: None,
        }
    }

    fn address(address_type: Option<&str>, smtp: Option<&str>) -> Address {
        Address {
            display_name: Some("n".to_string()),
            address_type: address_type.map(str::to_string),
            email_address: Some("/O=X".to_string()),
            smtp_address: smtp.map(str::to_string),
        }
    }

    #[test]
    fn rows_in_row_id_order_are_in_order_and_ties_do_not_count_as_a_difference() {
        assert_eq!(row_order(&[Some(0), Some(1), Some(2)]), RowOrder::InOrder);
        assert_eq!(row_order(&[Some(5), Some(5)]), RowOrder::InOrder);
        assert_eq!(row_order(&[Some(-1), Some(0)]), RowOrder::InOrder);
        assert_eq!(row_order(&[Some(2), Some(1)]), RowOrder::Differs);
        assert_eq!(
            row_order(&[Some(0), Some(2), Some(1), Some(3)]),
            RowOrder::Differs
        );
    }

    #[test]
    fn a_row_without_a_number_makes_the_order_unverifiable() {
        assert_eq!(row_order(&[Some(0), None]), RowOrder::Unverifiable);
        assert_eq!(row_order(&[None, Some(1), Some(0)]), RowOrder::Unverifiable);
    }

    #[test]
    fn the_order_is_counted_once_per_message_with_two_or_more_rows() {
        let mut notes = EnvelopeNotes::default();
        notes.note_order(&columns(true), &[]);
        notes.note_order(&columns(true), &[Some(0)]);
        assert_eq!(notes, EnvelopeNotes::default());

        notes.note_order(&columns(true), &[Some(0), Some(1)]);
        notes.note_order(&columns(true), &[Some(1), Some(0)]);
        notes.note_order(&columns(true), &[Some(1), None]);
        notes.note_order(&columns(false), &[None, None]);
        assert_eq!(notes.recipient_order_in_row_id_order, 1);
        assert_eq!(notes.recipient_order_differs_from_row_id, 1);
        assert_eq!(notes.recipient_order_unverifiable, 2);
    }

    #[test]
    fn missing_columns_are_counted_per_column() {
        let mut notes = EnvelopeNotes::default();
        notes.note_columns(&columns(false));
        notes.note_columns(&columns(true));
        assert_eq!(notes.recipient_tables_without_type_column, 0);
        assert_eq!(notes.recipient_tables_without_row_id_column, 1);
        assert_eq!(notes.recipient_tables_without_display_name_column, 0);
        assert_eq!(notes.recipient_tables_without_address_type_column, 2);
        assert_eq!(notes.recipient_tables_without_email_address_column, 0);
        assert_eq!(notes.recipient_tables_without_smtp_address_column, 2);
    }

    #[test]
    fn an_envelope_is_counted_by_what_it_carries() {
        let envelope = Envelope {
            sender: Some(address(Some("EX"), None)),
            sent_representing: None,
            recipients: vec![
                Recipient {
                    kind: RecipientKind::To,
                    address: address(Some("EX"), None),
                },
                Recipient {
                    kind: RecipientKind::Cc,
                    address: address(Some("EX"), Some("c@x")),
                },
                Recipient {
                    kind: RecipientKind::Bcc,
                    address: address(Some("SMTP"), None),
                },
            ],
            recipients_unlisted: 0,
            submit_time: Some(1),
            delivery_time: None,
            importance: Some(1),
            sensitivity: None,
            conversation_topic: Some("t".to_string()),
            conversation_index: None,
            transport_headers: Some("h".to_string()),
        };
        let mut notes = EnvelopeNotes::default();
        notes.note_envelope(&envelope);
        notes.note_envelope(&Envelope::default());
        assert_eq!(notes.messages, 2);
        assert_eq!(notes.sender_present, 1);
        assert_eq!(notes.sent_representing_present, 0);
        assert_eq!(notes.submit_time_present, 1);
        assert_eq!(notes.delivery_time_present, 0);
        assert_eq!(notes.importance_present, 1);
        assert_eq!(notes.sensitivity_present, 0);
        assert_eq!(notes.conversation_topic_present, 1);
        assert_eq!(notes.conversation_index_present, 0);
        assert_eq!(notes.transport_headers_present, 1);
        // The sender and the first recipient: EX with no SMTP address.
        assert_eq!(notes.exchange_without_smtp, 2);
    }

    #[test]
    fn the_report_derives_missing_and_unaccounted_and_lists_every_key_once() {
        let notes = EnvelopeNotes {
            messages: 5,
            sender_present: 3,
            recipient_rows: 9,
            recipients_to: 4,
            recipients_cc: 2,
            recipients_bcc: 1,
            recipients_unlisted_originator: 1,
            recipients_unlisted_other_type: 0,
            recipients_unlisted_type_unreadable: 1,
            ..EnvelopeNotes::default()
        };
        let lines = notes.report_lines();
        let get = |key: &str| lines.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        assert_eq!(get("sender_missing"), Some(2));
        assert_eq!(get("recipients_listed"), Some(7));
        assert_eq!(get("recipients_unlisted"), Some(2));
        assert_eq!(get("recipient_rows_unaccounted"), Some(0));

        let off = EnvelopeNotes {
            recipient_rows: 3,
            recipients_to: 1,
            ..EnvelopeNotes::default()
        };
        let off_lines = off.report_lines();
        let unaccounted = off_lines
            .iter()
            .find(|(k, _)| *k == "recipient_rows_unaccounted")
            .map(|(_, v)| *v);
        assert_eq!(unaccounted, Some(2));
        let over = EnvelopeNotes {
            recipient_rows: 1,
            recipients_to: 3,
            ..EnvelopeNotes::default()
        };
        assert!(
            over.report_lines()
                .iter()
                .any(|(k, v)| *k == "recipient_rows_unaccounted" && *v == 2)
        );

        let keys: BTreeSet<&str> = lines.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), lines.len());
    }

    #[test]
    fn every_counter_field_reaches_the_report() {
        // Setting each field to a distinct non-zero value must show up in some report line, so a
        // counter that is collected but never printed fails here.
        let notes = EnvelopeNotes {
            messages: 1,
            sender_present: 2,
            sent_representing_present: 3,
            submit_time_present: 4,
            delivery_time_present: 5,
            importance_present: 6,
            sensitivity_present: 7,
            conversation_topic_present: 8,
            conversation_index_present: 9,
            transport_headers_present: 10,
            exchange_without_smtp: 11,
            recipient_tables_absent: 12,
            recipient_tables_without_type_column: 13,
            recipient_tables_without_display_name_column: 14,
            recipient_tables_without_address_type_column: 15,
            recipient_tables_without_email_address_column: 16,
            recipient_tables_without_smtp_address_column: 17,
            recipient_tables_without_row_id_column: 18,
            recipient_rows: 19,
            recipient_rows_unreadable: 20,
            recipients_to: 21,
            recipients_cc: 22,
            recipients_bcc: 23,
            recipients_unlisted_originator: 24,
            recipients_unlisted_other_type: 25,
            recipients_unlisted_type_unreadable: 26,
            recipient_order_in_row_id_order: 27,
            recipient_order_differs_from_row_id: 28,
            recipient_order_unverifiable: 29,
        };
        let values: BTreeSet<u64> = notes.report_lines().iter().map(|(_, v)| *v).collect();
        for expected in 1..=29u64 {
            assert!(
                values.contains(&expected),
                "field {expected} is not printed"
            );
        }
    }
}
