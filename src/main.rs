//! `tsp`: privacy-safe inventory of Outlook `.pst` and `.msg` files (the teaspoon miner).

mod cli;
mod dry_run;
mod msg_report;
mod naming;
mod oxmsg_classify;
mod oxmsg_decode;
mod oxmsg_extract;
mod oxmsg_structure;
mod plan;
mod pst;
mod shared;
mod source_msg;
mod source_pst;
#[cfg(test)]
mod tests;
mod verify;

use anyhow::Result;
use clap::Parser;

use crate::cli::{classify_input, Args, InputKind};
use crate::dry_run::run_dry_run;
use crate::oxmsg_extract::run_msg_extract;
use crate::pst::run_pst_diagnostic;
use crate::verify::run_msg_verify;

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(out) = &args.out {
        if !args.dry_run {
            anyhow::bail!("exporting is not implemented yet; add --dry-run to plan an export");
        }
        return run_dry_run(&args.input, out);
    }

    match classify_input(&args.input)? {
        InputKind::Pst => run_pst_diagnostic(&args.input),
        InputKind::Msg {
            files,
            subdirectories_skipped,
        } => {
            // The custom MS-OXMSG extraction path is the .msg path (M3f).
            // `msg_parser` survives only as the independent oracle behind
            // --verify (ADR: custom MS-OXMSG parser graduates to the
            // production MSG path). The transitional --oxmsg and --extract
            // flags were retired in M3g.
            if args.verify {
                run_msg_verify(&files, subdirectories_skipped)
            } else {
                run_msg_extract(&files, subdirectories_skipped)
            }
        }
    }
}
