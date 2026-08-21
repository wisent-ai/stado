//! `stado doctor` — ordered deployment preflight.
//!
//! NO Python original: the Python CLI has no preflight command. See the
//! module docs of [`crate::doctor`] for what each probe interrogates and
//! why; this file is only the operator surface — argument parsing, the
//! table, and the exit code.
//!
//! `doctor` takes flags rather than subcommands, so the payload here is a
//! clap [`Args`] struct where the sibling command modules carry a
//! `Subcommand` enum. Registration in [`super`] is the same three lines
//! either way: one `pub mod`, one variant, one dispatch arm.
//!
//! The exit code is what makes the command scriptable: any FAIL exits
//! non-zero, so `stado doctor && stado coordinator` is a usable
//! deployment gate.

use clap::Args;

use crate::doctor::{self, Check, Report, Status};

use super::CmdError;

/// `stado doctor` flags.
#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Emit the full machine-readable report instead of the table. The
    /// JSON always carries every remedy, so `--fix-hints` adds nothing
    /// to it.
    #[arg(long)]
    json: bool,
    /// Also print the governing env var or command for checks that PASS,
    /// not only for the ones that need fixing.
    #[arg(long)]
    fix_hints: bool,
    /// Gate deployment prerequisites only. Fleet-wide service placement is
    /// still reported by ordinary `stado doctor`, but does not block installing
    /// an unrelated Stado control-plane release.
    #[arg(long)]
    deployment_preflight: bool,
    /// Verify only that the exact public release coordinate is served after
    /// deployment.
    #[arg(long, conflicts_with = "deployment_preflight")]
    release_verification: bool,
}

pub async fn dispatch(args: DoctorArgs) -> Result<(), CmdError> {
    let mut report = doctor::run().await;
    if args.deployment_preflight {
        report
            .checks
            .retain(|check| check.id != "placement" && check.id != "release");
    } else if args.release_verification {
        report.checks.retain(|check| check.id == "release");
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        print_human(&report, args.fix_hints);
    }

    // Non-zero on any FAIL, naming the earliest one: the later checks are
    // usually downstream of it, so that is the line to act on first. The
    // report is already on stdout; this only adds the verdict.
    match report.first_failure() {
        None => Ok(()),
        Some(first) => Err(CmdError::click(format!(
            "{} of {} checks FAILED; first blocking failure is {} ({}). {}",
            report.failed(),
            report.checks.len(),
            first.id,
            first.title,
            first.remedy
        ))),
    }
}

fn print_human(report: &Report, fix_hints: bool) {
    let rows: Vec<Vec<String>> = report
        .checks
        .iter()
        .map(|check| {
            vec![
                check.id.to_string(),
                check.status.label().to_string(),
                check.detail.clone(),
            ]
        })
        .collect();
    super::table::print(&["CHECK", "STATUS", "DETAIL"], &rows);

    // Remedies are the actionable half, so they get their own block rather
    // than a fourth column that would wrap the table past any terminal.
    // Default: only what needs fixing, in preflight order. With
    // --fix-hints: every check, so the knob behind a currently-passing one
    // is visible before it breaks.
    let shown: Vec<&Check> = report
        .checks
        .iter()
        .filter(|check| fix_hints || check.status != Status::Pass)
        .collect();
    if !shown.is_empty() {
        println!("\nREMEDIES");
        for check in shown {
            println!("  {} [{}] {}", check.id, check.status.label(), check.remedy);
        }
    }

    println!(
        "\n{} check(s): {} failed, {} warned, generated at {}.",
        report.checks.len(),
        report.failed(),
        report.warned(),
        report.generated_at
    );
    if report.status() == Status::Pass {
        println!("Deployment preflight is clean.");
    }
}
