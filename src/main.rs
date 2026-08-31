use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use outlook_pst::{
    messaging::{folder::Folder as PstFolder, message::Message as PstMessage, store::Store},
    ndb::node_id::NodeId,
};

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

    Ok(())
}

#[derive(Default)]
struct Totals {
    folders: u64,
    messages: u64,
    message_open_errors: u64,
    folder_open_errors: u64,
    property_values: u64,
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
}
