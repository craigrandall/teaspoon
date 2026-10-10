//! `tsp`: privacy-safe inventory and Markdown export of Outlook `.pst` and `.msg` files (the teaspoon miner).

mod archive;
mod cli;
mod dry_run;
mod export;
mod model;
mod msg_report;
mod naming;
mod oxmsg_classify;
mod oxmsg_decode;
mod oxmsg_envelope;
mod oxmsg_extract;
mod oxmsg_structure;
mod plan;
mod pst;
mod rtf_deencap;
mod shared;
mod source_msg;
mod source_pst;
#[cfg(test)]
mod tests;
mod verify;
mod verify_deencap;
mod verify_envelope;

use std::io::IsTerminal;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Args, InputKind, classify_input, collect_msg_files_recursive};
use crate::dry_run::run_dry_run;
use crate::export::{ExportOptions, ExportStatus, prompt_for_consent, run_export};
use crate::oxmsg_extract::run_msg_extract;
use crate::pst::run_pst_diagnostic;
use crate::verify::run_msg_verify;
use crate::verify_deencap::run_deencap_verify;
use crate::verify_envelope::run_envelope_verify;

fn main() -> Result<()> {
    let args = Args::parse();

    if args.recursive && !(args.verify_envelope || args.verify_deencap) {
        anyhow::bail!("--recursive applies only to --verify-envelope and --verify-deencap");
    }

    if let Some(out) = &args.out {
        if args.dry_run {
            return run_dry_run(&args.input, out);
        }
        let options = ExportOptions {
            overwrite: args.overwrite,
            source_hash: !args.no_source_hash,
            interactive: std::io::stdin().is_terminal(),
        };
        return match run_export(&args.input, out, &options, &mut prompt_for_consent)? {
            ExportStatus::Completed => Ok(()),
            // Nothing was written; the reason was printed. Exit code 2 is "refused", as
            // distinct from 1 (an error).
            ExportStatus::Refused => std::process::exit(2),
        };
    }

    match classify_input(&args.input)? {
        InputKind::Pst => run_pst_diagnostic(&args.input),
        InputKind::Msg {
            files,
            subdirectories_skipped,
        } => {
            // With --recursive a directory input is scanned at any depth, so nothing is skipped.
            let (files, subdirectories_skipped) = if args.recursive && args.input.is_dir() {
                (collect_msg_files_recursive(&args.input)?, 0)
            } else {
                (files, subdirectories_skipped)
            };
            // The custom MS-OXMSG extraction path is the .msg path (M3f).
            // `msg_parser` survives only as the independent oracle behind
            // --verify (ADR: custom MS-OXMSG parser graduates to the
            // production MSG path). The transitional --oxmsg and --extract
            // flags were retired in M3g.
            if args.verify {
                run_msg_verify(&files, subdirectories_skipped)
            } else if args.verify_envelope {
                run_envelope_verify(&files, subdirectories_skipped)
            } else if args.verify_deencap {
                run_deencap_verify(&files, subdirectories_skipped, args.dump_deencap.as_deref())
            } else {
                run_msg_extract(&files, subdirectories_skipped)
            }
        }
    }
}
