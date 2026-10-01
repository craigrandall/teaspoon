//! Command-line arguments and input classification.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

// =============================================================================
// CLI and input classification
// =============================================================================

#[derive(Debug, Parser)]
#[command(
    name = "tsp",
    about = "Privacy-safe inventory for the teaspoon Outlook message miner (PST or MSG)"
)]
pub(crate) struct Args {
    /// A .pst file, a single .msg file, or a directory of .msg files to inspect.
    pub(crate) input: PathBuf,

    /// Compare the custom MS-OXMSG extraction path against `msg_parser`
    /// for the same .msg input. Reads real property content internally to
    /// do the comparison, but prints only match/mismatch counts -- never
    /// the values compared. Also runs the custom path's structural
    /// accounting (every CFB entry classified, every properties stream
    /// and value stream decoded) and prints its gate counters plus
    /// `structural_gate_violations` (0 on a clean corpus). When any gate
    /// is nonzero it also prints the privacy-safe structural breakdown
    /// needed to triage it. PST input is unaffected.
    #[arg(long)]
    pub(crate) verify: bool,
}

pub(crate) enum InputKind {
    Pst,
    Msg {
        files: Vec<PathBuf>,
        /// Subdirectories found directly inside the scanned directory but
        /// not descended into (the scan is deliberately non-recursive).
        /// Surfaced in diagnostic output so this scoping choice is visible
        /// in the output, not just in the source.
        subdirectories_skipped: u64,
    },
}

/// Classifies the input by extension (or, for a directory, by scanning for
/// `.msg` files directly inside it -- not recursive). Never includes the
/// input path itself in any error message: paths can disclose information
/// about the user or their mailbox.
pub(crate) fn classify_input(path: &Path) -> Result<InputKind> {
    if path.is_dir() {
        let mut files = Vec::new();
        let mut subdirectories_skipped = 0u64;

        for entry in std::fs::read_dir(path)
            .context("failed to read input directory")?
            .filter_map(|entry| entry.ok())
        {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                subdirectories_skipped += 1;
                continue;
            }
            let is_msg = entry_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("msg"))
                .unwrap_or(false);
            if is_msg {
                files.push(entry_path);
            }
        }
        files.sort();
        if files.is_empty() {
            anyhow::bail!("input directory contains no .msg files");
        }
        return Ok(InputKind::Msg {
            files,
            subdirectories_skipped,
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("pst") => Ok(InputKind::Pst),
        Some("msg") => Ok(InputKind::Msg {
            files: vec![path.to_path_buf()],
            subdirectories_skipped: 0,
        }),
        _ => anyhow::bail!(
            "unsupported input: expected a .pst file, a .msg file, or a directory of .msg files"
        ),
    }
}
