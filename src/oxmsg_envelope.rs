//! Envelope extraction (M4d) from the custom MS-OXMSG layer: sender, sent-representing, To / Cc /
//! Bcc recipients, submit and delivery times, importance, sensitivity, the transport headers, and
//! the conversation fields. Reads real content into memory and returns it as model types; prints
//! nothing.
//!
//! Property identifiers and their confirmation status are listed in
//! `docs/plans/m4d-envelope-properties.md`.

use std::path::{Path, PathBuf};

use crate::dry_run::{PROP_DELIVERY_TIME, PROP_SUBMIT_TIME};
use crate::model::{Address, Envelope, Recipient, RecipientKind};
use crate::oxmsg_classify::{OxmsgEntryKind, OxmsgEntryScope, top_level_storage_paths};
use crate::oxmsg_decode::{
    DecodedFixedValue, decode_fixed_value, decode_properties_stream, extract_string8_codepage,
    properties_stream_header_len, read_stream_bytes,
};
use crate::oxmsg_extract::read_string_property;
use crate::shared::{CompoundFile, PROP_RECIPIENT_TYPE};

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

const PT_BINARY: u16 = 0x0102;
const PT_LONG: u16 = 0x0003;
const PT_SYSTIME: u16 = 0x0040;

const RECIPIENT_TYPE_TO: i32 = 1;
const RECIPIENT_TYPE_CC: i32 = 2;
const RECIPIENT_TYPE_BCC: i32 = 3;

/// Flags that Microsoft's `PidTagRecipientType` page says may be combined with the recipient type:
/// MAPI_P1 (a resend, 0x10000000) and MAPI_SUBMITTED (already received, 0x80000000). The type
/// itself is the value without them. The two numeric values are from a third-party library's
/// documentation, not from Microsoft text seen in this work; a value with any other high bit set
/// is still not listed (it is counted in `recipients_unlisted`).
const RECIPIENT_TYPE_FLAGS: i32 = 0x1000_0000 | i32::MIN;

/// The recipient type with the optional flags removed.
fn recipient_type_without_flags(value: i32) -> i32 {
    value & !RECIPIENT_TYPE_FLAGS
}

/// The identifiers of one address's four properties.
struct AddressProps {
    name: u16,
    address_type: u16,
    email_address: u16,
    smtp_address: u16,
}

const SENDER_PROPS: AddressProps = AddressProps {
    name: PROP_SENDER_NAME,
    address_type: PROP_SENDER_ADDRESS_TYPE,
    email_address: PROP_SENDER_EMAIL_ADDRESS,
    smtp_address: PROP_SENDER_SMTP_ADDRESS,
};

const SENT_REPRESENTING_PROPS: AddressProps = AddressProps {
    name: PROP_SENT_REPRESENTING_NAME,
    address_type: PROP_SENT_REPRESENTING_ADDRESS_TYPE,
    email_address: PROP_SENT_REPRESENTING_EMAIL_ADDRESS,
    smtp_address: PROP_SENT_REPRESENTING_SMTP_ADDRESS,
};

const RECIPIENT_PROPS: AddressProps = AddressProps {
    name: PROP_RECIPIENT_DISPLAY_NAME,
    address_type: PROP_RECIPIENT_ADDRESS_TYPE,
    email_address: PROP_RECIPIENT_EMAIL_ADDRESS,
    smtp_address: PROP_RECIPIENT_SMTP_ADDRESS,
};

/// A string property, or `None` when absent or empty.
fn read_text(comp: &mut CompoundFile, base: &Path, id: u16, codepage: u32) -> Option<String> {
    read_string_property(comp, base, id, codepage).filter(|s| !s.is_empty())
}

/// An address read from the properties under `base`; `None` when none of the four is present.
fn read_address(
    comp: &mut CompoundFile,
    base: &Path,
    codepage: u32,
    props: &AddressProps,
) -> Option<Address> {
    let address = Address {
        display_name: read_text(comp, base, props.name, codepage),
        address_type: read_text(comp, base, props.address_type, codepage),
        email_address: read_text(comp, base, props.email_address, codepage),
        smtp_address: read_text(comp, base, props.smtp_address, codepage),
    };
    if address.is_empty() {
        None
    } else {
        Some(address)
    }
}

/// The fixed-size message properties the envelope needs, read from the properties stream.
struct FixedValues {
    submit_time: Option<i64>,
    delivery_time: Option<i64>,
    importance: Option<i32>,
    sensitivity: Option<i32>,
}

fn read_fixed_values(comp: &mut CompoundFile, base: &Path, header_len: usize) -> FixedValues {
    let mut out = FixedValues {
        submit_time: None,
        delivery_time: None,
        importance: None,
        sensitivity: None,
    };
    let Some(bytes) = read_stream_bytes(comp, &base.join("__properties_version1.0")) else {
        return out;
    };
    let Some(decoded) = decode_properties_stream(&bytes, header_len) else {
        return out;
    };
    for entry in &decoded.entries {
        match (entry.property_id, entry.property_type) {
            (PROP_SUBMIT_TIME, PT_SYSTIME) | (PROP_DELIVERY_TIME, PT_SYSTIME) => {
                if let Some(DecodedFixedValue::SysTime(ticks)) =
                    decode_fixed_value(entry.property_type, &entry.tail)
                {
                    let ticks = i64::try_from(ticks).ok();
                    if entry.property_id == PROP_SUBMIT_TIME {
                        out.submit_time = ticks;
                    } else {
                        out.delivery_time = ticks;
                    }
                }
            }
            (PROP_IMPORTANCE, PT_LONG) => {
                if let Some(DecodedFixedValue::Long(v)) =
                    decode_fixed_value(entry.property_type, &entry.tail)
                {
                    out.importance = Some(v);
                }
            }
            (PROP_SENSITIVITY, PT_LONG) => {
                if let Some(DecodedFixedValue::Long(v)) =
                    decode_fixed_value(entry.property_type, &entry.tail)
                {
                    out.sensitivity = Some(v);
                }
            }
            _ => {}
        }
    }
    out
}

/// The recipient type (`PidTagRecipientType`) of one recipient storage, as stored (with any
/// optional flags still set).
fn read_recipient_type(comp: &mut CompoundFile, recipient_path: &Path) -> Option<i32> {
    let bytes = read_stream_bytes(comp, &recipient_path.join("__properties_version1.0"))?;
    let decoded = decode_properties_stream(&bytes, 8)?;
    decoded.entries.iter().find_map(|entry| {
        if entry.property_id != PROP_RECIPIENT_TYPE || entry.property_type != PT_LONG {
            return None;
        }
        match decode_fixed_value(entry.property_type, &entry.tail) {
            Some(DecodedFixedValue::Long(v)) => Some(v),
            _ => None,
        }
    })
}

/// To, Cc, and Bcc recipients in storage order, plus the number of storages that are none of
/// those (the originator row, an unknown type, or an unreadable type). The optional MAPI_P1 and
/// MAPI_SUBMITTED flags are ignored when classifying.
fn read_recipients(comp: &mut CompoundFile, codepage: u32) -> (Vec<Recipient>, usize) {
    let mut paths: Vec<PathBuf> = top_level_storage_paths(&*comp, OxmsgEntryKind::RecipientStorage)
        .into_iter()
        .collect();
    paths.sort();

    let mut recipients = Vec::new();
    let mut unlisted = 0usize;
    for path in paths {
        let kind = match read_recipient_type(comp, &path).map(recipient_type_without_flags) {
            Some(RECIPIENT_TYPE_TO) => RecipientKind::To,
            Some(RECIPIENT_TYPE_CC) => RecipientKind::Cc,
            Some(RECIPIENT_TYPE_BCC) => RecipientKind::Bcc,
            _ => {
                unlisted += 1;
                continue;
            }
        };
        let address = read_address(comp, &path, codepage, &RECIPIENT_PROPS).unwrap_or_default();
        recipients.push(Recipient { kind, address });
    }
    (recipients, unlisted)
}

/// Reads the envelope of the message whose own streams live at the root of `comp`.
pub(crate) fn extract_envelope(comp: &mut CompoundFile) -> Envelope {
    let base = Path::new("/");
    let header_len = properties_stream_header_len(OxmsgEntryScope::Message).unwrap_or(32);
    let codepage = extract_string8_codepage(comp, base, header_len).codepage;

    let fixed = read_fixed_values(comp, base, header_len);
    let (recipients, recipients_unlisted) = read_recipients(comp, codepage);
    let conversation_index = read_stream_bytes(
        comp,
        &base.join(format!(
            "__substg1.0_{PROP_CONVERSATION_INDEX:04X}{PT_BINARY:04X}"
        )),
    )
    .filter(|bytes| !bytes.is_empty());

    Envelope {
        sender: read_address(comp, base, codepage, &SENDER_PROPS),
        sent_representing: read_address(comp, base, codepage, &SENT_REPRESENTING_PROPS),
        recipients,
        recipients_unlisted,
        submit_time: fixed.submit_time,
        delivery_time: fixed.delivery_time,
        importance: fixed.importance,
        sensitivity: fixed.sensitivity,
        conversation_topic: read_text(comp, base, PROP_CONVERSATION_TOPIC, codepage),
        conversation_index,
        transport_headers: read_text(comp, base, PROP_TRANSPORT_HEADERS, codepage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entry(ty: u16, id: u16, value: [u8; 8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&value);
        v
    }

    fn long_entry(id: u16, value: i32) -> Vec<u8> {
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&value.to_le_bytes());
        entry(PT_LONG, id, bytes)
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    fn put_text(comp: &mut cfb::CompoundFile<std::fs::File>, base: &str, id: u16, text: &str) {
        put(
            comp,
            &format!("{base}/__substg1.0_{id:04X}001F"),
            &utf16(text),
        );
    }

    fn add_recipient(
        comp: &mut cfb::CompoundFile<std::fs::File>,
        index: u32,
        recipient_type: Option<i32>,
        name: &str,
        address_type: Option<&str>,
        email: Option<&str>,
        smtp: Option<&str>,
    ) {
        let base = format!("/__recip_version1.0_#{index:08X}");
        comp.create_storage(&base)
            .expect("create recipient storage");
        let mut props = vec![0u8; 8];
        if let Some(t) = recipient_type {
            props.extend(long_entry(PROP_RECIPIENT_TYPE, t));
        }
        put(comp, &format!("{base}/__properties_version1.0"), &props);
        put_text(comp, &base, PROP_RECIPIENT_DISPLAY_NAME, name);
        if let Some(v) = address_type {
            put_text(comp, &base, PROP_RECIPIENT_ADDRESS_TYPE, v);
        }
        if let Some(v) = email {
            put_text(comp, &base, PROP_RECIPIENT_EMAIL_ADDRESS, v);
        }
        if let Some(v) = smtp {
            put_text(comp, &base, PROP_RECIPIENT_SMTP_ADDRESS, v);
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-env-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("m.msg")
    }

    #[test]
    fn a_message_with_nothing_has_an_empty_envelope() {
        let path = temp_file("empty");
        let mut comp = cfb::create(&path).expect("create CFB");
        put(&mut comp, "/__properties_version1.0", &[0u8; 32]);
        drop(comp);
        let mut comp = cfb::open(&path).expect("open CFB");
        assert_eq!(extract_envelope(&mut comp), Envelope::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_full_envelope_is_read() {
        let path = temp_file("full");
        let mut comp = cfb::create(&path).expect("create CFB");

        let mut props = vec![0u8; 32];
        props.extend(entry(
            PT_SYSTIME,
            PROP_SUBMIT_TIME,
            133_000_000_000_000_000u64.to_le_bytes(),
        ));
        props.extend(entry(
            PT_SYSTIME,
            PROP_DELIVERY_TIME,
            133_000_000_100_000_000u64.to_le_bytes(),
        ));
        props.extend(long_entry(PROP_IMPORTANCE, 2));
        props.extend(long_entry(PROP_SENSITIVITY, 1));
        put(&mut comp, "/__properties_version1.0", &props);

        put_text(&mut comp, "", PROP_SENDER_NAME, "Alice Sender");
        put_text(&mut comp, "", PROP_SENDER_ADDRESS_TYPE, "SMTP");
        put_text(
            &mut comp,
            "",
            PROP_SENDER_EMAIL_ADDRESS,
            "alice@example.com",
        );
        put_text(&mut comp, "", PROP_SENDER_SMTP_ADDRESS, "alice@example.com");
        put_text(&mut comp, "", PROP_SENT_REPRESENTING_NAME, "Team Mailbox");
        put_text(
            &mut comp,
            "",
            PROP_TRANSPORT_HEADERS,
            "Received: from x\r\nSubject: Hi",
        );
        put_text(&mut comp, "", PROP_CONVERSATION_TOPIC, "Topic");
        put(
            &mut comp,
            &format!("/__substg1.0_{PROP_CONVERSATION_INDEX:04X}{PT_BINARY:04X}"),
            &[1, 2, 0xAB],
        );

        add_recipient(
            &mut comp,
            0,
            Some(1),
            "Bob Receiver",
            Some("SMTP"),
            Some("bob@example.com"),
            None,
        );
        add_recipient(
            &mut comp,
            1,
            Some(2),
            "Dan",
            Some("EX"),
            Some("/O=X/CN=DAN"),
            None,
        );
        add_recipient(
            &mut comp,
            2,
            Some(3),
            "Eve",
            None,
            None,
            Some("eve@example.com"),
        );
        add_recipient(&mut comp, 3, Some(0), "Alice Sender", None, None, None);
        add_recipient(&mut comp, 4, None, "No type", None, None, None);
        add_recipient(
            &mut comp,
            5,
            Some(1),
            "Second To",
            None,
            None,
            Some("to2@example.com"),
        );
        drop(comp);

        let mut comp = cfb::open(&path).expect("open CFB");
        let env = extract_envelope(&mut comp);

        let sender = env.sender.expect("sender");
        assert_eq!(sender.display_name.as_deref(), Some("Alice Sender"));
        assert_eq!(sender.address_type.as_deref(), Some("SMTP"));
        assert_eq!(sender.email_address.as_deref(), Some("alice@example.com"));
        assert_eq!(sender.smtp_address.as_deref(), Some("alice@example.com"));

        let represented = env.sent_representing.expect("sent representing");
        assert_eq!(represented.display_name.as_deref(), Some("Team Mailbox"));
        assert_eq!(represented.smtp_address, None);

        assert_eq!(env.submit_time, Some(133_000_000_000_000_000));
        assert_eq!(env.delivery_time, Some(133_000_000_100_000_000));
        assert_eq!(env.importance, Some(2));
        assert_eq!(env.sensitivity, Some(1));
        assert_eq!(env.conversation_topic.as_deref(), Some("Topic"));
        assert_eq!(env.conversation_index.as_deref(), Some(&[1u8, 2, 0xAB][..]));
        assert_eq!(
            env.transport_headers.as_deref(),
            Some("Received: from x\r\nSubject: Hi")
        );

        // Storage order, To/Cc/Bcc only; the originator row and the untyped row are counted.
        let kinds: Vec<RecipientKind> = env.recipients.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RecipientKind::To,
                RecipientKind::Cc,
                RecipientKind::Bcc,
                RecipientKind::To
            ]
        );
        assert_eq!(env.recipients_unlisted, 2);
        assert_eq!(
            env.recipients[0].address.display_name.as_deref(),
            Some("Bob Receiver")
        );
        assert_eq!(
            env.recipients[1].address.address_type.as_deref(),
            Some("EX")
        );
        assert_eq!(env.recipients[1].address.smtp_address, None);
        assert_eq!(
            env.recipients[2].address.smtp_address.as_deref(),
            Some("eve@example.com")
        );
        assert_eq!(
            env.recipients[3].address.display_name.as_deref(),
            Some("Second To")
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

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
    fn a_flagged_recipient_is_listed_and_an_unknown_flag_is_not() {
        let path = temp_file("flags");
        let mut comp = cfb::create(&path).expect("create CFB");
        put(&mut comp, "/__properties_version1.0", &[0u8; 32]);
        add_recipient(
            &mut comp,
            0,
            Some(1 | 0x1000_0000),
            "Resent To",
            None,
            None,
            Some("a@example.com"),
        );
        add_recipient(
            &mut comp,
            1,
            Some(2 | i32::MIN),
            "Submitted Cc",
            None,
            None,
            Some("b@example.com"),
        );
        add_recipient(
            &mut comp,
            2,
            Some(1 | 0x2000_0000),
            "Unknown flag",
            None,
            None,
            None,
        );
        drop(comp);

        let mut comp = cfb::open(&path).expect("open CFB");
        let env = extract_envelope(&mut comp);
        let kinds: Vec<RecipientKind> = env.recipients.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RecipientKind::To, RecipientKind::Cc]);
        assert_eq!(
            env.recipients[0].address.display_name.as_deref(),
            Some("Resent To")
        );
        assert_eq!(env.recipients_unlisted, 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn wrongly_typed_fixed_properties_are_ignored() {
        let path = temp_file("wrongtype");
        let mut comp = cfb::create(&path).expect("create CFB");
        let mut props = vec![0u8; 32];
        // Importance stored as a time, not a long: not read as an importance.
        props.extend(entry(PT_SYSTIME, PROP_IMPORTANCE, [0u8; 8]));
        put(&mut comp, "/__properties_version1.0", &props);
        drop(comp);
        let mut comp = cfb::open(&path).expect("open CFB");
        assert_eq!(extract_envelope(&mut comp).importance, None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
