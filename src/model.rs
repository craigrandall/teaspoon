//! The normalized model (M4b-4), grown stage by stage.
//!
//! Pure data with no adapter types, so the `.msg` reader (and later the PST reader) can fill it
//! and the archive renderer can read it without either knowing about the other. It holds what
//! the stages so far extract: the body forms as the source stores them (M4e-2b) and the
//! *envelope* (sender, recipients, times, importance, sensitivity, transport headers,
//! conversation fields). Attachments, embedded messages, the raw property bag, and named
//! properties join it when the stages that fill them (M4f, M4g) exist: a field nothing fills
//! would be dead code and a promise nothing keeps.

/// The body forms a message carries, as the source stores them. The reader fills this; the body
/// pipeline (`body.rs`) selects and converts from it, and the export keeps each form that exists
/// as an original file.
///
/// A form that is absent or zero-length is `None`, with one exception: `plain_text` is whatever
/// the property decoded to, so a zero-length value is `Some("")`. The archive keeps only
/// non-empty forms, and the export report counts a zero-length or whitespace-only plain body as
/// "empty" and a missing one as "absent".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BodyForms {
    /// `PidTagHtml` (`0x1013`, `PT_BINARY`): the native HTML bytes.
    pub(crate) native_html: Option<Vec<u8>>,
    /// `PidTagInternetCodepage` as stored (positive values only), the code page of the native
    /// HTML. Recorded in `metadata.json` whenever the message has one.
    pub(crate) native_html_code_page: Option<u32>,
    /// The decompressed RTF (`PidTagRtfCompressed`, decompressed).
    pub(crate) rtf: Option<Vec<u8>>,
    /// A non-empty `PidTagRtfCompressed` existed and could not be decompressed.
    pub(crate) rtf_decompression_failed: bool,
    /// `PidTagBody`, as read.
    pub(crate) plain_text: Option<String>,
}

/// One mail address as the source stores it. Nothing is derived or "fixed up": a display name
/// may be missing, the address type may be `SMTP`, `EX` (an Exchange X.500 distinguished name in
/// `email_address`), or anything else, and the SMTP form is a separate property that may be
/// absent. Renderers decide what to show; the model keeps what was there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Address {
    pub(crate) display_name: Option<String>,
    pub(crate) address_type: Option<String>,
    pub(crate) email_address: Option<String>,
    pub(crate) smtp_address: Option<String>,
}

impl Address {
    pub(crate) fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.address_type.is_none()
            && self.email_address.is_none()
            && self.smtp_address.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecipientKind {
    To,
    Cc,
    Bcc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recipient {
    pub(crate) kind: RecipientKind,
    pub(crate) address: Address,
}

/// The message's envelope. Every field is `None`/empty when the source holds nothing for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Envelope {
    /// The sender (`PidTagSender*`).
    pub(crate) sender: Option<Address>,
    /// The mailbox the message was sent on behalf of (`PidTagSentRepresenting*`).
    pub(crate) sent_representing: Option<Address>,
    /// To, Cc, and Bcc recipients in recipient-storage order.
    pub(crate) recipients: Vec<Recipient>,
    /// Recipient storages that are not To/Cc/Bcc: the originator row (type 0), an unknown type,
    /// or a type that could not be read. Counted, not listed.
    pub(crate) recipients_unlisted: usize,
    /// `PidTagClientSubmitTime` as FILETIME ticks.
    pub(crate) submit_time: Option<i64>,
    /// `PidTagMessageDeliveryTime` as FILETIME ticks.
    pub(crate) delivery_time: Option<i64>,
    /// `PidTagImportance` as the stored integer (0 low, 1 normal, 2 high).
    pub(crate) importance: Option<i32>,
    /// `PidTagSensitivity` as the stored integer (0 none, 1 personal, 2 private, 3 confidential).
    pub(crate) sensitivity: Option<i32>,
    pub(crate) conversation_topic: Option<String>,
    pub(crate) conversation_index: Option<Vec<u8>>,
    /// `PidTagTransportMessageHeaders`: the Internet headers, verbatim.
    pub(crate) transport_headers: Option<String>,
}

/// What the export reads from one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageContent {
    /// The subject with the U+0001 prefix marker removed.
    pub(crate) subject: Option<String>,
    pub(crate) internet_message_id: Option<String>,
    /// Delivery time, falling back to submit time, as FILETIME ticks (the planner's ordering time).
    pub(crate) time_filetime: Option<i64>,
    pub(crate) bodies: BodyForms,
    pub(crate) envelope: Envelope,
}
