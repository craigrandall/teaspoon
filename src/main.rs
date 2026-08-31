use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use outlook_pst::{
    messaging::{folder::Folder as PstFolder, message::Message as PstMessage, store::Store},
    ndb::node_id::NodeId,
};

/// MS-OXPROPS property identifiers used only to check *presence*, never to
/// read or print the value behind them.
///
/// - PidTagBody (plain text body)
/// - PidTagBodyHtml (HTML body)
/// - PidTagRtfCompressed (RTF body)
const PROP_BODY: u16 = 0x1000;
const PROP_BODY_HTML: u16 = 0x1013;
const PROP_RTF_COMPRESSED: u16 = 0x1009;

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe PST inventory for the teaspoon Outlook message miner"
)]
struct Args {
    /// PST file to inspect.
    pst: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let store = outlook_pst::open_store(&args.pst).context("failed to open PST")?;

    // Deliberately do not print the input path or PST display name. Both can
    // disclose information about the user or their mailbox.

    let ipm_id = store
        .properties()
        .ipm_sub_tree_entry_id()
        .context("PST has no IPM subtree entry ID")?;

    let ipm = store
        .open_folder(&ipm_id)
        .context("failed to open IPM subtree")?;

    println!("ipm_subtree=ok");

    let mut totals = Totals::default();
    walk_folder(store.as_ref(), ipm.as_ref(), &mut totals)?;

    println!("inventory=privacy_safe");
    println!("ipm_subtree=opened");
    println!("folders={}", totals.folders);
    println!("messages={}", totals.messages);
    println!("message_open_errors={}", totals.message_open_errors);
    println!("folder_open_errors={}", totals.folder_open_errors);
    println!("property_values={}", totals.property_values);

    // --- P4a: extended structural/aggregate diagnostics --------------------
    //
    // Everything below reports counts, and message-class *names*, which are
    // drawn from a bounded, standard MAPI vocabulary (e.g. "IPM.Note") rather
    // than user-authored content. Nothing below prints subjects, addresses,
    // bodies, filenames, or any other message content.
    println!(
        "message_class_read_errors={}",
        totals.message_class_read_errors
    );
    for (class, count) in &totals.message_classes {
        println!("message_class class={class} count={count}");
    }

    println!("bodies_plain={}", totals.bodies_plain);
    println!("bodies_html={}", totals.bodies_html);
    println!("bodies_rtf={}", totals.bodies_rtf);

    println!(
        "messages_with_recipients={}",
        totals.messages_with_recipients
    );
    println!("total_recipients={}", totals.total_recipients);
    println!("max_recipients_on_a_message={}", totals.max_recipients);

    println!(
        "messages_with_attachments={}",
        totals.messages_with_attachments
    );
    println!("total_attachments={}", totals.total_attachments);
    println!("max_attachments_on_a_message={}", totals.max_attachments);

    Ok(())
}

#[derive(Default)]
struct Totals {
    folders: u64,
    messages: u64,
    message_open_errors: u64,
    folder_open_errors: u64,
    property_values: u64,

    // P4a additions.
    message_classes: BTreeMap<String, u64>,
    message_class_read_errors: u64,

    bodies_plain: u64,
    bodies_html: u64,
    bodies_rtf: u64,

    messages_with_recipients: u64,
    total_recipients: u64,
    max_recipients: u64,

    messages_with_attachments: u64,
    total_attachments: u64,
    max_attachments: u64,
}

fn walk_folder(store: &dyn Store, folder: &dyn PstFolder, totals: &mut Totals) -> Result<()> {
    totals.folders += 1;

    if let Some(contents) = folder.contents_table() {
        for row in contents.rows_matrix() {
            totals.messages += 1;

            let entry_id = match store
                .properties()
                .make_entry_id(NodeId::from(u32::from(row.id())))
            {
                Ok(entry_id) => entry_id,
                Err(_) => {
                    totals.message_open_errors += 1;
                    continue;
                }
            };

            match store.open_message(&entry_id, None) {
                Ok(message) => inspect_message(message.as_ref(), totals),
                Err(_) => totals.message_open_errors += 1,
            }
        }
        // Folder names, message subjects, entry IDs, and other property values
        // are intentionally excluded from diagnostic output.
    }

    if let Some(hierarchy) = folder.hierarchy_table() {
        for row in hierarchy.rows_matrix() {
            let node = NodeId::from(u32::from(row.id()));

            let entry_id = match store.properties().make_entry_id(node) {
                Ok(entry_id) => entry_id,
                Err(_) => {
                    totals.folder_open_errors += 1;
                    continue;
                }
            };

            match store.open_folder(&entry_id) {
                Ok(child) => walk_folder(store, child.as_ref(), totals)?,
                Err(_) => totals.folder_open_errors += 1,
            }
        }
    }

    Ok(())
}

fn inspect_message(message: &dyn PstMessage, totals: &mut Totals) {
    let properties = message.properties();
    totals.property_values += properties.iter().count() as u64;

    record_message_class(totals, properties.message_class());

    record_body_flags(
        totals,
        properties.get(PROP_BODY).is_some(),
        properties.get(PROP_BODY_HTML).is_some(),
        properties.get(PROP_RTF_COMPRESSED).is_some(),
    );

    // Recipients: aggregate count only. Per-recipient detail (To/CC/BCC,
    // addresses, display names) is deliberately out of scope for this pass.
    // See docs/verification/m1-results.md for the follow-on work required
    // (column-level PidTagRecipientType reads) and why it was deferred.
    let recipient_count = message
        .recipient_table()
        .map(|table| table.rows_matrix().count() as u64)
        .unwrap_or(0);
    record_recipients(totals, recipient_count);

    // Attachments: aggregate count only. Zero-byte / embedded-message /
    // inline-image classification is deliberately out of scope for this
    // pass. See docs/verification/m1-results.md.
    let attachment_count = message
        .attachment_table()
        .map(|table| table.rows_matrix().count() as u64)
        .unwrap_or(0);
    record_attachments(totals, attachment_count);
}

/// Records a message-class observation. `class` is `Err` when the message
/// has no readable `PidTagMessageClass` property; that is counted separately
/// rather than silently dropped, consistent with the project's no-silent-loss
/// principle (ADR-0004).
fn record_message_class(totals: &mut Totals, class: std::io::Result<String>) {
    match class {
        Ok(class) => *totals.message_classes.entry(class).or_insert(0) += 1,
        Err(_) => totals.message_class_read_errors += 1,
    }
}

/// Records body-*availability* only. Never receives or touches actual body
/// content -- callers must pass presence booleans, not the property values.
fn record_body_flags(totals: &mut Totals, has_plain: bool, has_html: bool, has_rtf: bool) {
    if has_plain {
        totals.bodies_plain += 1;
    }
    if has_html {
        totals.bodies_html += 1;
    }
    if has_rtf {
        totals.bodies_rtf += 1;
    }
}

fn record_recipients(totals: &mut Totals, count: u64) {
    if count > 0 {
        totals.messages_with_recipients += 1;
    }
    totals.total_recipients += count;
    totals.max_recipients = totals.max_recipients.max(count);
}

fn record_attachments(totals: &mut Totals, count: u64) {
    if count > 0 {
        totals.messages_with_attachments += 1;
    }
    totals.total_attachments += count;
    totals.max_attachments = totals.max_attachments.max(count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_class_is_aggregated_by_name() {
        let mut totals = Totals::default();
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Note".to_string()));
        record_message_class(&mut totals, Ok("IPM.Note.SMIME".to_string()));

        assert_eq!(totals.message_classes.get("IPM.Note"), Some(&2));
        assert_eq!(totals.message_classes.get("IPM.Note.SMIME"), Some(&1));
        assert_eq!(totals.message_class_read_errors, 0);
    }

    #[test]
    fn unreadable_message_class_is_counted_not_dropped() {
        let mut totals = Totals::default();
        record_message_class(
            &mut totals,
            Err(std::io::Error::other("missing PidTagMessageClass")),
        );

        assert_eq!(totals.message_class_read_errors, 1);
        assert!(totals.message_classes.is_empty());
    }

    #[test]
    fn body_flags_are_presence_only() {
        let mut totals = Totals::default();
        record_body_flags(&mut totals, true, true, false);
        record_body_flags(&mut totals, true, false, false);

        assert_eq!(totals.bodies_plain, 2);
        assert_eq!(totals.bodies_html, 1);
        assert_eq!(totals.bodies_rtf, 0);
    }

    #[test]
    fn recipient_counts_track_presence_total_and_max() {
        let mut totals = Totals::default();
        record_recipients(&mut totals, 0);
        record_recipients(&mut totals, 3);
        record_recipients(&mut totals, 1);

        assert_eq!(totals.messages_with_recipients, 2);
        assert_eq!(totals.total_recipients, 4);
        assert_eq!(totals.max_recipients, 3);
    }

    #[test]
    fn attachment_counts_track_presence_total_and_max() {
        let mut totals = Totals::default();
        record_attachments(&mut totals, 0);
        record_attachments(&mut totals, 2);
        record_attachments(&mut totals, 5);

        assert_eq!(totals.messages_with_attachments, 2);
        assert_eq!(totals.total_attachments, 7);
        assert_eq!(totals.max_attachments, 5);
    }
}
