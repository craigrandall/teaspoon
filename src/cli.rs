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
    about = "Privacy-safe inventory and Markdown export for Outlook PST and MSG files (the teaspoon miner)"
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

    /// Compare the envelope the export extracts (subject, sender, To/Cc/Bcc recipients) against
    /// `msg_parser` for the same .msg input, field by field. Reads real content internally and
    /// prints only match/mismatch counts, plus counts of which envelope fields the corpus
    /// carries -- never the values.
    #[arg(long, conflicts_with_all = ["verify", "out", "dry_run"])]
    pub(crate) verify_envelope: bool,

    /// Directory to export into. The archive is written to `<DIR>/<input name>/`: one
    /// directory per message holding `message.md` and `metadata.json`, a `folder.json` in every
    /// folder, and a `folder.json` at the archive root that marks the directory as created
    /// by `tsp`. Exporting `.msg` input works; a `.pst` export arrives with M4i. Standard output
    /// stays content-free (counts only). With `--dry-run` nothing is written.
    #[arg(long, value_name = "DIR", conflicts_with = "verify")]
    pub(crate) out: Option<PathBuf>,

    /// Plan an export of the input into `--out` without writing anything,
    /// and print only content-free counts (the naming census): how many
    /// names need sanitizing or shortening, how many collide, how long the
    /// longest path is, and whether every planned path fits the Windows
    /// budget. A directory input is planned recursively, mirroring its
    /// subdirectories as folders.
    #[arg(long, requires = "out", conflicts_with = "verify")]
    pub(crate) dry_run: bool,

    /// Consent, without asking, to replace files that an earlier export by `tsp` generated
    /// and that this export would change. Without it, `tsp` asks (interactive terminal) or
    /// stops with exit code 2 (not interactive). It never touches files it did not generate and
    /// never writes into a directory it did not create.
    #[arg(long, requires = "out", conflicts_with = "dry_run")]
    pub(crate) overwrite: bool,

    /// Do not compute the SHA-256 of the source file(s) recorded in the archive root's
    /// `folder.json`.
    #[arg(long, requires = "out")]
    pub(crate) no_source_hash: bool,
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
