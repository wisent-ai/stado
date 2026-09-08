//! Command-line surface of `stado repair`.

use clap::Args;

use crate::cli::CmdError;

/// `stado repair list`, `stado repair show SERVICE STEP`, or
/// `stado repair SERVICE`. The first positional is deliberately not a clap
/// subcommand: service names are declaration data, so a new service never adds
/// an enum variant or another command.
#[derive(Debug, Args)]
pub(crate) struct RepairArgs {
    /// `list`, `show`, or the declared service name to repair.
    #[arg(value_name = "COMMAND_OR_SERVICE")]
    pub(super) command_or_service: String,
    /// SERVICE after `show`.
    #[arg(value_name = "SERVICE")]
    pub(super) service_argument: Option<String>,
    /// STEP after `show`.
    #[arg(value_name = "STEP")]
    pub(super) step_argument: Option<String>,
    /// Limit `list` to one declared service.
    #[arg(long, value_name = "NAME")]
    pub(super) service: Option<String>,
    /// Run only this declared step.
    #[arg(long, value_name = "STEP")]
    pub(super) step: Option<String>,
    /// Registry host on which to observe or apply the repair.
    #[arg(long, value_name = "TARGET")]
    pub(super) target: Option<String>,
    /// Apply the declared repair; omission is a read-only report.
    #[arg(long)]
    pub(super) apply: bool,
    /// Emit one machine-readable report.
    #[arg(long)]
    pub(super) json: bool,
}

pub(super) fn reject_extra(args: &RepairArgs, operation: &str) -> Result<(), CmdError> {
    if args.step.is_some() || args.target.is_some() || args.apply {
        return Err(CmdError::usage(format!(
            "repair {operation} accepts only its documented declaration filters and --json."
        )));
    }
    Ok(())
}
