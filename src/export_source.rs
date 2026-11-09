//! The two seams the exporter has for a source (M4i, decision D1).
//!
//! The exporter plans from a source tree, then opens each message and streams each attachment
//! one at a time. Until M4i the only source was `.msg`; these types let a PST be the other
//! without forking the staging, consent, read-back, or naming code, which therefore stay as
//! verified.
//!
//! * [`ExportSources`] says where a planned message comes from: a `.msg` file the planner
//!   found, or a message node of the open PST.
//! * [`AttachmentReader`] streams the data of one message's top-level attachments and opens
//!   embedded messages. A PST reader opens none (an embedded-message or OLE attachment cannot
//!   be read through the crate), so the exporter records those as not extracted.
//! * [`DataOpen`] is what opening one attachment's data gives: a stream, or the reason there is
//!   none. A reason may carry a content-free label of the crate's error, counted in the report.

use std::io::Read;

use crate::pst_export::{PstAttachments, PstSource};
use crate::source_msg::{EmbeddedOpen, MsgAttachments, MsgFileMap};

/// Where the planned messages of an export come from.
pub(crate) enum ExportSources {
    Msg(MsgFileMap),
    Pst(PstSource),
}

/// The outcome of opening one attachment's data.
pub(crate) enum DataOpen<'a> {
    /// The data as a stream, with the length the source declares for it.
    Stream {
        data: Box<dyn Read + 'a>,
        declared: u64,
    },
    /// There is no data to read: the stream is missing (`.msg`), or the data property is absent
    /// or null (PST).
    Missing { label: Option<String> },
    /// There is data that could not be read.
    ReadFailed { label: Option<String> },
}

/// Streams one message's top-level attachments by position (the order of the planner's source
/// attachments and of the message content's descriptors).
pub(crate) enum AttachmentReader<'a> {
    Msg(MsgAttachments<'a>),
    Pst(PstAttachments),
}

impl AttachmentReader<'_> {
    /// The data of attachment `index`.
    pub(crate) fn open_data(&mut self, index: usize) -> DataOpen<'_> {
        match self {
            AttachmentReader::Msg(reader) => match reader.open_data(index) {
                Some((stream, declared)) => DataOpen::Stream {
                    data: Box::new(stream),
                    declared,
                },
                None => DataOpen::Missing { label: None },
            },
            AttachmentReader::Pst(reader) => reader.open_data(index),
        }
    }

    /// Attachment `index` opened as an embedded message. Only a `.msg` reader can: a PST reader
    /// returns `None` for every index.
    pub(crate) fn open_embedded(&mut self, index: usize) -> Option<EmbeddedOpen<'_>> {
        match self {
            AttachmentReader::Msg(reader) => reader.open_embedded(index),
            AttachmentReader::Pst(_) => None,
        }
    }
}
