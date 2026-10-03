//! Seed of the normalized model (the part of M4b-4 that the M4c walking skeleton needs).
//!
//! Pure data with no adapter types, so it can later grow into the full `OutlookItem` without
//! touching the `.msg` or PST readers. Only the fields the first end-to-end export consumes
//! exist here; recipients, headers, attachments, and the raw property bag arrive with M4d-M4g.

/// The plain-text body as the source holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlainBody {
    /// No `PidTagBody` value.
    Absent,
    /// A value that is empty or only whitespace.
    Empty,
    Text(String),
}

/// Which other body forms the message carries. M4c does not convert them, so the export
/// records their presence instead of dropping them silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyAvailability {
    pub(crate) html_native: bool,
    pub(crate) html_via_rtf: bool,
    pub(crate) rtf: bool,
}

/// What the export reads from one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageContent {
    /// The subject with the U+0001 prefix marker removed.
    pub(crate) subject: Option<String>,
    pub(crate) internet_message_id: Option<String>,
    /// Delivery time, falling back to submit time, as FILETIME ticks.
    pub(crate) time_filetime: Option<i64>,
    pub(crate) plain_body: PlainBody,
    pub(crate) bodies: BodyAvailability,
}
