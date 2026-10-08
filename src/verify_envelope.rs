//! Envelope differential verification (`--verify-envelope`, M4d): the envelope the export
//! extracts, compared field by field with `msg_parser` for the same `.msg` files.
//!
//! Like `--verify`, this reads real content internally and prints only match/mismatch counts,
//! plus counts of which envelope fields the files carry. Fields `msg_parser` does not expose
//! (sent-representing, times, importance, sensitivity, conversation fields, transport headers)
//! cannot be compared; they are reported only as presence counts, and the properties document
//! says so.
//!
//! What is compared: the subject (after removing the subject-prefix marker from both), the
//! sender's name and email, and the To / Cc / Bcc recipient lists (length, then name and email
//! position by position). `msg_parser`'s single email string is checked against both
//! `PidTagEmailAddress` and the SMTP address, and the report says which one it matched, because
//! which property `msg_parser` reads is not documented. Since v0.1.26 the report also separates,
//! within "matched only one", whether the other property was absent or present and different, so
//! it is visible whether `message.md` (which prefers the SMTP address) can ever disagree with
//! `msg_parser`.

use std::path::PathBuf;

use anyhow::Result;
use msg_parser::Outlook;

use crate::dry_run::strip_subject_marker;
use crate::model::{Address, Envelope, RecipientKind};
use crate::source_msg::read_message_content;
use crate::verify::{CountComparison, CountTally, compare_count, print_count_tally};

/// Which of the custom path's two email properties equals `msg_parser`'s email string.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EmailComparison {
    /// Equal to both (the usual case when they are the same string, or both are empty).
    Both,
    EmailAddressOnly,
    SmtpOnly,
    Neither,
}

pub(crate) fn compare_email(msg_parser: &str, address: &Address) -> EmailComparison {
    let email_address = address.email_address.as_deref().unwrap_or("");
    let smtp_address = address.smtp_address.as_deref().unwrap_or("");
    match (msg_parser == email_address, msg_parser == smtp_address) {
        (true, true) => EmailComparison::Both,
        (true, false) => EmailComparison::EmailAddressOnly,
        (false, true) => EmailComparison::SmtpOnly,
        (false, false) => EmailComparison::Neither,
    }
}

pub(crate) fn compare_text(msg_parser: &str, custom: &str) -> CountComparison {
    if msg_parser == custom {
        CountComparison::Match
    } else {
        CountComparison::Mismatch
    }
}

/// An absent property, or one stored as an empty string.
fn is_blank(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(str::is_empty)
}

#[derive(Default)]
pub(crate) struct EmailTally {
    pub(crate) both: u64,
    /// Of `both`: the two properties are both absent or empty (and `msg_parser`'s string is empty).
    pub(crate) both_empty: u64,
    pub(crate) email_address_only: u64,
    /// Of `email_address_only`: the SMTP property is absent or empty.
    pub(crate) email_address_only_smtp_absent: u64,
    /// Of `email_address_only`: the SMTP property is present and differs. `message.md` shows the
    /// SMTP address first, so each of these is a place it can disagree with `msg_parser`.
    pub(crate) email_address_only_smtp_present_different: u64,
    pub(crate) smtp_only: u64,
    /// Of `smtp_only`: `PidTagEmailAddress` is absent or empty.
    pub(crate) smtp_only_email_address_absent: u64,
    /// Of `smtp_only`: `PidTagEmailAddress` is present and differs (typically an Exchange DN).
    pub(crate) smtp_only_email_address_present_different: u64,
    pub(crate) neither: u64,
}

impl EmailTally {
    pub(crate) fn record(&mut self, comparison: EmailComparison, address: &Address) {
        match comparison {
            EmailComparison::Both => {
                self.both += 1;
                if is_blank(&address.email_address) && is_blank(&address.smtp_address) {
                    self.both_empty += 1;
                }
            }
            EmailComparison::EmailAddressOnly => {
                self.email_address_only += 1;
                if is_blank(&address.smtp_address) {
                    self.email_address_only_smtp_absent += 1;
                } else {
                    self.email_address_only_smtp_present_different += 1;
                }
            }
            EmailComparison::SmtpOnly => {
                self.smtp_only += 1;
                if is_blank(&address.email_address) {
                    self.smtp_only_email_address_absent += 1;
                } else {
                    self.smtp_only_email_address_present_different += 1;
                }
            }
            EmailComparison::Neither => self.neither += 1,
        }
    }
}

fn print_email_tally(name: &str, tally: &EmailTally) {
    println!("{name}_match_both={}", tally.both);
    println!("{name}_match_both_empty={}", tally.both_empty);
    println!(
        "{name}_match_email_address_only={}",
        tally.email_address_only
    );
    println!(
        "{name}_match_email_address_only_smtp_absent={}",
        tally.email_address_only_smtp_absent
    );
    println!(
        "{name}_match_email_address_only_smtp_present_different={}",
        tally.email_address_only_smtp_present_different
    );
    println!("{name}_match_smtp_only={}", tally.smtp_only);
    println!(
        "{name}_match_smtp_only_email_address_absent={}",
        tally.smtp_only_email_address_absent
    );
    println!(
        "{name}_match_smtp_only_email_address_present_different={}",
        tally.smtp_only_email_address_present_different
    );
    println!("{name}_mismatch={}", tally.neither);
}

#[derive(Default)]
pub(crate) struct EnvelopeVerifyTotals {
    pub(crate) open_errors_msg_parser: u64,
    pub(crate) open_errors_custom: u64,
    pub(crate) subject: CountTally,
    pub(crate) sender_name: CountTally,
    pub(crate) sender_email: EmailTally,
    pub(crate) recipient_list_to: CountTally,
    pub(crate) recipient_list_cc: CountTally,
    pub(crate) recipient_list_bcc: CountTally,
    pub(crate) recipient_name: CountTally,
    pub(crate) recipient_email: EmailTally,
    // Custom-path-only presence counts: nothing in `msg_parser` to compare them with.
    pub(crate) sender_present_total: u64,
    pub(crate) sent_representing_present_total: u64,
    pub(crate) submit_time_present_total: u64,
    pub(crate) delivery_time_present_total: u64,
    pub(crate) importance_present_total: u64,
    pub(crate) sensitivity_present_total: u64,
    pub(crate) transport_headers_present_total: u64,
    pub(crate) conversation_topic_present_total: u64,
    pub(crate) conversation_index_present_total: u64,
    pub(crate) recipients_unlisted_total: u64,
    /// Addresses of type `EX` with no SMTP address: the only email form is an Exchange
    /// distinguished name, which `message.md` does not show.
    pub(crate) exchange_without_smtp_total: u64,
    /// Messages whose sender has no display name or no usable email at all.
    pub(crate) sender_missing_total: u64,
}

fn compare_recipient_lists(
    msg_parser: &[(&str, &str)],
    custom: &[&Address],
    lists: &mut CountTally,
    names: &mut CountTally,
    emails: &mut EmailTally,
) {
    lists.record(compare_count(msg_parser.len() as u64, custom.len() as u64));
    for (mp, address) in msg_parser.iter().zip(custom.iter()) {
        names.record(compare_text(
            mp.0,
            address.display_name.as_deref().unwrap_or(""),
        ));
        emails.record(compare_email(mp.1, address), address);
    }
}

fn recipients_of(envelope: &Envelope, kind: RecipientKind) -> Vec<&Address> {
    envelope
        .recipients
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| &r.address)
        .collect()
}

fn is_exchange_without_smtp(address: &Address) -> bool {
    address
        .address_type
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("EX"))
        && address.smtp_address.is_none()
}

pub(crate) fn collect_envelope_verify_totals(files: &[PathBuf]) -> EnvelopeVerifyTotals {
    let mut totals = EnvelopeVerifyTotals::default();
    for file in files {
        let outlook = match Outlook::from_path(file) {
            Ok(outlook) => outlook,
            Err(_) => {
                totals.open_errors_msg_parser += 1;
                continue;
            }
        };
        let content = match read_message_content(file) {
            Ok(content) => content,
            Err(_) => {
                totals.open_errors_custom += 1;
                continue;
            }
        };
        let envelope = &content.envelope;

        totals.subject.record(compare_text(
            strip_subject_marker(&outlook.subject).0,
            content.subject.as_deref().unwrap_or(""),
        ));

        let no_sender = Address::default();
        let sender = envelope.sender.as_ref().unwrap_or(&no_sender);
        totals.sender_name.record(compare_text(
            &outlook.sender.name,
            sender.display_name.as_deref().unwrap_or(""),
        ));
        totals
            .sender_email
            .record(compare_email(&outlook.sender.email, sender), sender);

        let to: Vec<(&str, &str)> = outlook
            .to
            .iter()
            .map(|p| (p.name.as_str(), p.email.as_str()))
            .collect();
        let cc: Vec<(&str, &str)> = outlook
            .cc
            .iter()
            .map(|p| (p.name.as_str(), p.email.as_str()))
            .collect();
        let bcc: Vec<(&str, &str)> = outlook
            .bcc
            .iter()
            .map(|p| (p.name.as_str(), p.email.as_str()))
            .collect();
        compare_recipient_lists(
            &to,
            &recipients_of(envelope, RecipientKind::To),
            &mut totals.recipient_list_to,
            &mut totals.recipient_name,
            &mut totals.recipient_email,
        );
        compare_recipient_lists(
            &cc,
            &recipients_of(envelope, RecipientKind::Cc),
            &mut totals.recipient_list_cc,
            &mut totals.recipient_name,
            &mut totals.recipient_email,
        );
        compare_recipient_lists(
            &bcc,
            &recipients_of(envelope, RecipientKind::Bcc),
            &mut totals.recipient_list_bcc,
            &mut totals.recipient_name,
            &mut totals.recipient_email,
        );

        totals.sender_present_total += u64::from(envelope.sender.is_some());
        totals.sent_representing_present_total += u64::from(envelope.sent_representing.is_some());
        totals.submit_time_present_total += u64::from(envelope.submit_time.is_some());
        totals.delivery_time_present_total += u64::from(envelope.delivery_time.is_some());
        totals.importance_present_total += u64::from(envelope.importance.is_some());
        totals.sensitivity_present_total += u64::from(envelope.sensitivity.is_some());
        totals.transport_headers_present_total += u64::from(envelope.transport_headers.is_some());
        totals.conversation_topic_present_total += u64::from(envelope.conversation_topic.is_some());
        totals.conversation_index_present_total += u64::from(envelope.conversation_index.is_some());
        totals.recipients_unlisted_total += envelope.recipients_unlisted as u64;
        totals.exchange_without_smtp_total += envelope
            .recipients
            .iter()
            .map(|r| &r.address)
            .chain(envelope.sender.iter())
            .filter(|a| is_exchange_without_smtp(a))
            .count() as u64;
        if envelope.sender.as_ref().is_none_or(|s| {
            s.display_name.is_none() && s.smtp_address.is_none() && s.email_address.is_none()
        }) {
            totals.sender_missing_total += 1;
        }
    }
    totals
}

pub(crate) fn print_envelope_verify_report(totals: &EnvelopeVerifyTotals) {
    println!("open_errors_msg_parser={}", totals.open_errors_msg_parser);
    println!("open_errors_custom={}", totals.open_errors_custom);
    print_count_tally("subject", &totals.subject);
    print_count_tally("sender_name", &totals.sender_name);
    print_email_tally("sender_email", &totals.sender_email);
    print_count_tally("recipient_list_to", &totals.recipient_list_to);
    print_count_tally("recipient_list_cc", &totals.recipient_list_cc);
    print_count_tally("recipient_list_bcc", &totals.recipient_list_bcc);
    print_count_tally("recipient_name", &totals.recipient_name);
    print_email_tally("recipient_email", &totals.recipient_email);
    println!("sender_present_total={}", totals.sender_present_total);
    println!("sender_missing_total={}", totals.sender_missing_total);
    println!(
        "sent_representing_present_total={}",
        totals.sent_representing_present_total
    );
    println!(
        "submit_time_present_total={}",
        totals.submit_time_present_total
    );
    println!(
        "delivery_time_present_total={}",
        totals.delivery_time_present_total
    );
    println!(
        "importance_present_total={}",
        totals.importance_present_total
    );
    println!(
        "sensitivity_present_total={}",
        totals.sensitivity_present_total
    );
    println!(
        "transport_headers_present_total={}",
        totals.transport_headers_present_total
    );
    println!(
        "conversation_topic_present_total={}",
        totals.conversation_topic_present_total
    );
    println!(
        "conversation_index_present_total={}",
        totals.conversation_index_present_total
    );
    println!(
        "recipients_unlisted_total={}",
        totals.recipients_unlisted_total
    );
    println!(
        "exchange_without_smtp_total={}",
        totals.exchange_without_smtp_total
    );
}

pub(crate) fn run_envelope_verify(files: &[PathBuf], subdirectories_skipped: u64) -> Result<()> {
    let totals = collect_envelope_verify_totals(files);
    println!("inventory=privacy_safe");
    println!("input_kind=msg_envelope_verify");
    println!("files_scanned={}", files.len());
    println!("subdirectories_skipped={subdirectories_skipped}");
    print_envelope_verify_report(&totals);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(email: Option<&str>, smtp: Option<&str>) -> Address {
        Address {
            display_name: None,
            address_type: None,
            email_address: email.map(str::to_string),
            smtp_address: smtp.map(str::to_string),
        }
    }

    #[test]
    fn an_email_is_matched_against_both_custom_properties() {
        assert_eq!(
            compare_email("a@x", &address(Some("a@x"), Some("a@x"))),
            EmailComparison::Both
        );
        assert_eq!(
            compare_email("a@x", &address(Some("a@x"), Some("b@x"))),
            EmailComparison::EmailAddressOnly
        );
        assert_eq!(
            compare_email("a@x", &address(Some("/O=EX"), Some("a@x"))),
            EmailComparison::SmtpOnly
        );
        assert_eq!(
            compare_email("a@x", &address(Some("c@x"), None)),
            EmailComparison::Neither
        );
        // Both sides empty counts as agreement.
        assert_eq!(
            compare_email("", &Address::default()),
            EmailComparison::Both
        );
    }

    #[test]
    fn the_tally_separates_an_absent_property_from_a_different_one() {
        let mut tally = EmailTally::default();

        let absent = address(Some("a@x"), None);
        tally.record(compare_email("a@x", &absent), &absent);
        let empty = address(Some("a@x"), Some(""));
        tally.record(compare_email("a@x", &empty), &empty);
        let different = address(Some("a@x"), Some("b@x"));
        tally.record(compare_email("a@x", &different), &different);
        assert_eq!(tally.email_address_only, 3);
        assert_eq!(tally.email_address_only_smtp_absent, 2);
        assert_eq!(tally.email_address_only_smtp_present_different, 1);

        let no_address = address(None, Some("a@x"));
        tally.record(compare_email("a@x", &no_address), &no_address);
        let exchange = address(Some("/O=EX"), Some("a@x"));
        tally.record(compare_email("a@x", &exchange), &exchange);
        assert_eq!(tally.smtp_only, 2);
        assert_eq!(tally.smtp_only_email_address_absent, 1);
        assert_eq!(tally.smtp_only_email_address_present_different, 1);

        let same = address(Some("a@x"), Some("a@x"));
        tally.record(compare_email("a@x", &same), &same);
        let nothing = Address::default();
        tally.record(compare_email("", &nothing), &nothing);
        assert_eq!(tally.both, 2);
        assert_eq!(tally.both_empty, 1);

        let other = address(Some("c@x"), None);
        tally.record(compare_email("a@x", &other), &other);
        assert_eq!(tally.neither, 1);
    }

    #[test]
    fn text_is_compared_exactly() {
        assert_eq!(compare_text("a", "a"), CountComparison::Match);
        assert_eq!(compare_text("a", "A"), CountComparison::Mismatch);
        assert_eq!(compare_text("", ""), CountComparison::Match);
    }

    #[test]
    fn recipient_lists_are_compared_by_length_then_position() {
        let one = Address {
            display_name: Some("Bob".to_string()),
            address_type: None,
            email_address: Some("b@x".to_string()),
            smtp_address: None,
        };
        let two = Address {
            display_name: Some("Carol".to_string()),
            address_type: None,
            email_address: None,
            smtp_address: Some("c@x".to_string()),
        };
        let mut lists = CountTally::default();
        let mut names = CountTally::default();
        let mut emails = EmailTally::default();
        compare_recipient_lists(
            &[("Bob", "b@x"), ("Carl", "c@x")],
            &[&one, &two],
            &mut lists,
            &mut names,
            &mut emails,
        );
        assert_eq!((lists.matched, lists.mismatched), (1, 0));
        assert_eq!((names.matched, names.mismatched), (1, 1));
        assert_eq!(emails.both, 0);
        assert_eq!(emails.email_address_only, 1);
        assert_eq!(emails.email_address_only_smtp_absent, 1);
        assert_eq!(emails.smtp_only, 1);
        assert_eq!(emails.smtp_only_email_address_absent, 1);

        // A shorter custom list is a list mismatch; only the common prefix is compared.
        let mut lists = CountTally::default();
        let mut names = CountTally::default();
        let mut emails = EmailTally::default();
        compare_recipient_lists(
            &[("Bob", "b@x"), ("Carl", "c@x")],
            &[&one],
            &mut lists,
            &mut names,
            &mut emails,
        );
        assert_eq!((lists.matched, lists.mismatched), (0, 1));
        assert_eq!(names.matched, 1);
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
        assert!(!is_exchange_without_smtp(&address(Some("a@x"), None)));
    }
}
