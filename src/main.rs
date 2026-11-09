//! `tsp`: privacy-safe inventory and Markdown export of Outlook `.pst` and `.msg` files (the teaspoon miner).

mod archive;
mod bakeoff;
mod body;
mod cli;
mod dry_run;
mod export;
mod export_source;
mod layout;
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
mod pst_attachments;
mod pst_export;
mod pst_password;
mod pst_text;
#[cfg(test)]
mod robustness_tests;
mod rtf_deencap;
mod shared;
mod source_msg;
mod source_pst;
mod spike_pst_attachments;
mod stage;
#[cfg(test)]
mod tests;
mod verify;
mod verify_attachments;
mod verify_bodies;
mod verify_deencap;
mod verify_envelope;

use std::io::IsTerminal;

use anyhow::Result;
use clap::Parser;

use crate::bakeoff::run_bakeoff;
use crate::cli::{Args, InputKind, classify_input_with};
use crate::dry_run::run_dry_run;
use crate::export::{ExportOptions, ExportStatus, prompt_for_consent, run_export};
use crate::oxmsg_extract::run_msg_extract;
use crate::pst::run_pst_diagnostic;
use crate::pst_password::PasswordRefusal;
use crate::spike_pst_attachments::run_spike_attachments;
use crate::verify::run_msg_verify;
use crate::verify_attachments::run_attachments_verify;
use crate::verify_bodies::run_bodies_verify;
use crate::verify_deencap::run_deencap_verify;
use crate::verify_envelope::run_envelope_verify;

/// Exit status of a run that failed: something broke (a file would not open, the input was not
/// usable, a store property had an unexpected type).
const EXIT_ERROR: i32 = 1;

/// Exit status of a run that understood the input and declined to proceed, so that it read or
/// wrote nothing the caller asked for, and the caller can fix the cause: an export that needs
/// consent, and a password-protected PST whose password is missing or wrong.
const EXIT_REFUSED: i32 = 2;

fn main() {
    if let Err(err) = run() {
        // The text the default `fn main() -> Result<()>` printed, so standard error is unchanged.
        eprintln!("Error: {err:?}");
        std::process::exit(exit_code(&err));
    }
}

/// The exit status of a failed run: [`EXIT_REFUSED`] for a password refusal (found whatever
/// context was added to it), [`EXIT_ERROR`] for every other error.
fn exit_code(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<PasswordRefusal>().is_some() {
        EXIT_REFUSED
    } else {
        EXIT_ERROR
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    if args.recursive
        && !(args.verify_envelope
            || args.verify_deencap
            || args.verify_bodies
            || args.verify_attachments)
    {
        anyhow::bail!(
            "--recursive applies only to --verify-envelope, --verify-deencap, --verify-bodies, and --verify-attachments"
        );
    }

    if args.bakeoff_md {
        // The input is a directory of recovered HTML, not a mail container.
        return run_bakeoff(&args.input, args.bakeoff_dump.as_deref());
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
            // Nothing was written; the reason was printed. Exit status 2 is "refused", as
            // distinct from 1 (an error).
            ExportStatus::Refused => std::process::exit(EXIT_REFUSED),
        };
    }

    match classify_input_with(&args.input, args.recursive)? {
        InputKind::Pst => {
            if args.verify_bodies {
                anyhow::bail!("--verify-bodies applies only to .msg input");
            }
            if args.verify_attachments {
                anyhow::bail!("--verify-attachments applies only to .msg input");
            }
            if args.spike_attachments {
                return run_spike_attachments(&args.input, args.spike_hash_file.as_deref());
            }
            run_pst_diagnostic(&args.input)
        }
        InputKind::Msg {
            files,
            subdirectories_skipped,
        } => {
            // (With --recursive, `classify_input_with` has already scanned the directory down to
            // the depth cap and counted the directories below it in `subdirectories_skipped`.)
            if args.spike_attachments {
                anyhow::bail!("--spike-attachments applies only to .pst input");
            }
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
            } else if args.verify_bodies {
                run_bodies_verify(&files, subdirectories_skipped, args.dump_bodies.as_deref())
            } else if args.verify_attachments {
                run_attachments_verify(&files, subdirectories_skipped)
            } else {
                run_msg_extract(&files, subdirectories_skipped)
            }
        }
    }
}

#[cfg(test)]
mod exit_status_tests {
    use anyhow::{Context, anyhow};

    use super::*;

    #[test]
    fn a_password_refusal_exits_2_bare_or_with_context() {
        for refusal in [
            PasswordRefusal::Missing,
            PasswordRefusal::Wrong,
            PasswordRefusal::UnsupportedEncoding,
        ] {
            let bare = anyhow::Error::new(refusal);
            assert_eq!(exit_code(&bare), EXIT_REFUSED, "{refusal:?}");
            let wrapped = Err::<(), anyhow::Error>(bare)
                .context("while opening the PST")
                .unwrap_err();
            assert_eq!(
                exit_code(&wrapped),
                EXIT_REFUSED,
                "{refusal:?} with context"
            );
        }
    }

    #[test]
    fn any_other_error_exits_1() {
        assert_eq!(exit_code(&anyhow!("failed to open PST")), EXIT_ERROR);
        assert_eq!(
            exit_code(&anyhow!(
                "PidTagPstPassword has an unexpected type; cannot verify the PST password"
            )),
            EXIT_ERROR
        );
        assert_eq!(
            exit_code(&anyhow!("TSP_PST_PASSWORD is not valid Unicode")),
            EXIT_ERROR
        );
    }

    #[test]
    fn the_two_statuses_are_distinct_and_not_success() {
        assert_ne!(EXIT_ERROR, EXIT_REFUSED);
        assert_ne!(EXIT_ERROR, 0);
        assert_ne!(EXIT_REFUSED, 0);
    }
}
