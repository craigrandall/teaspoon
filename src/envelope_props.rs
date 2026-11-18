//! Envelope property identifiers and the rules both readers share (M4i, drop B).
//!
//! The `.msg` reader (`oxmsg_envelope.rs`) and the PST reader (`pst_envelope.rs`) fill the same
//! model [`Envelope`](crate::model::Envelope) from the same MAPI properties. What those
//! properties are, which four of them make up one address, how a stored `PidTagRecipientType`
//! becomes To, Cc, or Bcc, and what makes an address an Exchange address with no SMTP form are
//! therefore defined once, here, so the two readers cannot drift apart. Before drop B these
//! lived in `oxmsg_envelope.rs`, with a private copy of the To / Cc / Bcc values that
//! `shared.rs` already had.
//!
//! The confirmation status of each identifier is in `docs/plans/m4d-envelope-properties.md`.

use crate::model::{Address, RecipientKind};
use crate::shared::{
    RECIPIENT_TYPE_BCC, RECIPIENT_TYPE_CC, RECIPIENT_TYPE_ORIG, RECIPIENT_TYPE_TO,
};

// Message-level properties (MS-OXPROPS; confirmation status in the M4d properties document).
pub(crate) const PROP_IMPORTANCE: u16 = 0x0017;
pub(crate) const PROP_SENSITIVITY: u16 = 0x0036;
pub(crate) const PROP_TRANSPORT_HEADERS: u16 = 0x007D;
pub(crate) const PROP_CONVERSATION_TOPIC: u16 = 0x0070;
pub(crate) const PROP_CONVERSATION_INDEX: u16 = 0x0071;

// The sender, as the transport recorded it.
pub(crate) const PROP_SENDER_NAME: u16 = 0x0C1A;
pub(crate) const PROP_SENDER_ADDRESS_TYPE: u16 = 0x0C1E;
pub(crate) const PROP_SENDER_EMAIL_ADDRESS: u16 = 0x0C1F;
pub(crate) const PROP_SENDER_SMTP_ADDRESS: u16 = 0x5D01;

// The mailbox the message was sent on behalf of.
pub(crate) const PROP_SENT_REPRESENTING_NAME: u16 = 0x0042;
pub(crate) const PROP_SENT_REPRESENTING_ADDRESS_TYPE: u16 = 0x0064;
pub(crate) const PROP_SENT_REPRESENTING_EMAIL_ADDRESS: u16 = 0x0065;
pub(crate) const PROP_SENT_REPRESENTING_SMTP_ADDRESS: u16 = 0x5D02;

// A recipient row.
pub(crate) const PROP_RECIPIENT_DISPLAY_NAME: u16 = 0x3001;
pub(crate) const PROP_RECIPIENT_ADDRESS_TYPE: u16 = 0x3002;
pub(crate) const PROP_RECIPIENT_EMAIL_ADDRESS: u16 = 0x3003;
pub(crate) const PROP_RECIPIENT_SMTP_ADDRESS: u16 = 0x39FE;

/// `PidTagRowid`, a recipient row's own number. The PST reader uses it only to count how often
/// the recipient table's row order is not the row-number order; nothing is sorted by it. Not yet
/// checked against Microsoft's page (tier C in the M4d vocabulary: the author's knowledge of the
/// MAPI headers).
pub(crate) const PROP_RECIPIENT_ROW_ID: u16 = 0x3000;

/// The identifiers of one address's four properties.
pub(crate) struct AddressProps {
    pub(crate) name: u16,
    pub(crate) address_type: u16,
    pub(crate) email_address: u16,
    pub(crate) smtp_address: u16,
}

pub(crate) const SENDER_PROPS: AddressProps = AddressProps {
    name: PROP_SENDER_NAME,
    address_type: PROP_SENDER_ADDRESS_TYPE,
    email_address: PROP_SENDER_EMAIL_ADDRESS,
    smtp_address: PROP_SENDER_SMTP_ADDRESS,
};

pub(crate) const SENT_REPRESENTING_PROPS: AddressProps = AddressProps {
    name: PROP_SENT_REPRESENTING_NAME,
    address_type: PROP_SENT_REPRESENTING_ADDRESS_TYPE,
    email_address: PROP_SENT_REPRESENTING_EMAIL_ADDRESS,
    smtp_address: PROP_SENT_REPRESENTING_SMTP_ADDRESS,
};

pub(crate) const RECIPIENT_PROPS: AddressProps = AddressProps {
    name: PROP_RECIPIENT_DISPLAY_NAME,
    address_type: PROP_RECIPIENT_ADDRESS_TYPE,
    email_address: PROP_RECIPIENT_EMAIL_ADDRESS,
    smtp_address: PROP_RECIPIENT_SMTP_ADDRESS,
};

/// Flags that Microsoft's `PidTagRecipientType` page says may be combined with the recipient type:
/// MAPI_P1 (a resend, 0x10000000) and MAPI_SUBMITTED (already received, 0x80000000). The type
/// itself is the value without them. The two numeric values are from a third-party library's
/// documentation, not from Microsoft text seen in this work; a value with any other high bit set
/// is still not listed (it is counted as unlisted by both readers).
const RECIPIENT_TYPE_FLAGS: i32 = 0x1000_0000 | i32::MIN;

/// The recipient type with the optional flags removed.
fn recipient_type_without_flags(value: i32) -> i32 {
    value & !RECIPIENT_TYPE_FLAGS
}

/// To, Cc, or Bcc for a stored `PidTagRecipientType`, ignoring the optional flags; `None` for
/// the originator row (type 0), an unknown type, or a flag that is not one of the two.
pub(crate) fn recipient_kind(stored: i32) -> Option<RecipientKind> {
    match recipient_type_without_flags(stored) {
        RECIPIENT_TYPE_TO => Some(RecipientKind::To),
        RECIPIENT_TYPE_CC => Some(RecipientKind::Cc),
        RECIPIENT_TYPE_BCC => Some(RecipientKind::Bcc),
        _ => None,
    }
}

/// True for the originator row (type 0, with or without the optional flags).
pub(crate) fn is_originator_row(stored: i32) -> bool {
    recipient_type_without_flags(stored) == RECIPIENT_TYPE_ORIG
}

/// An address of type `EX` with no SMTP address: the only email form it has is an Exchange
/// distinguished name, which `message.md` does not show.
pub(crate) fn is_exchange_without_smtp(address: &Address) -> bool {
    address
        .address_type
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("EX"))
        && address.smtp_address.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_optional_recipient_type_flags_are_ignored_when_classifying() {
        assert_eq!(recipient_type_without_flags(1), 1);
        assert_eq!(recipient_type_without_flags(1 | 0x1000_0000), 1);
        assert_eq!(recipient_type_without_flags(2 | i32::MIN), 2);
        assert_eq!(recipient_type_without_flags(3 | 0x1000_0000 | i32::MIN), 3);
        // A bit that is not one of the two flags is kept, so the value is not a known type.
        assert_eq!(
            recipient_type_without_flags(1 | 0x2000_0000),
            1 | 0x2000_0000
        );
    }

    #[test]
    fn a_stored_type_becomes_to_cc_or_bcc_and_nothing_else_does() {
        assert_eq!(recipient_kind(1), Some(RecipientKind::To));
        assert_eq!(recipient_kind(2), Some(RecipientKind::Cc));
        assert_eq!(recipient_kind(3), Some(RecipientKind::Bcc));
        assert_eq!(recipient_kind(2 | 0x1000_0000), Some(RecipientKind::Cc));
        assert_eq!(recipient_kind(3 | i32::MIN), Some(RecipientKind::Bcc));
        assert_eq!(recipient_kind(0), None);
        assert_eq!(recipient_kind(4), None);
        assert_eq!(recipient_kind(-1), None);
        assert_eq!(recipient_kind(1 | 0x2000_0000), None);
    }

    #[test]
    fn only_type_zero_is_the_originator_row() {
        assert!(is_originator_row(0));
        assert!(is_originator_row(0x1000_0000));
        assert!(is_originator_row(i32::MIN));
        assert!(!is_originator_row(1));
        assert!(!is_originator_row(0x2000_0000));
    }

    #[test]
    fn exchange_addresses_without_smtp_are_recognized() {
        let ex = Address {
            display_name: None,
            address_type: Some("ex".to_string()),
            email_address: Some("/O=X".to_string()),
            smtp_address: None,
        };
        assert!(is_exchange_without_smtp(&ex));
        let with_smtp = Address {
            smtp_address: Some("a@x".to_string()),
            ..ex.clone()
        };
        assert!(!is_exchange_without_smtp(&with_smtp));
        let smtp = Address {
            address_type: Some("SMTP".to_string()),
            ..ex
        };
        assert!(!is_exchange_without_smtp(&smtp));
        assert!(!is_exchange_without_smtp(&Address::default()));
    }

    #[test]
    fn the_three_address_sets_are_distinct() {
        let sets = [&SENDER_PROPS, &SENT_REPRESENTING_PROPS, &RECIPIENT_PROPS];
        let mut ids: Vec<u16> = sets
            .iter()
            .flat_map(|p| [p.name, p.address_type, p.email_address, p.smtp_address])
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total);
        assert!(!ids.contains(&PROP_RECIPIENT_ROW_ID));
    }
}
