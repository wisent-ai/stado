//! `stado release quarantine ...` — list the digests one host refuses to roll
//! out again, and retire exactly one of them.
//!
//! The release agent quarantines a digest that failed to become ready and then
//! never tries it again, which is correct: a candidate that dies in ninety
//! seconds must not be respawned in a loop. What was missing was the way back.
//! Until this command existed there were exactly two: hand-edit
//! `<state_dir>/<product>.json` on the host, or publish a new version number so
//! the digest changes. The operator refused both, and both deserve refusing —
//! the first is an unaudited write to the file a rollout is driven from, and the
//! second burns a version to say "try again".
//!
//! What this is not: `clear` starts nothing, restarts nothing and kills nothing.
//! It removes one map entry. The agent's next tick reads the state file, finds
//! the desired digest no longer quarantined, and rolls it out on its own — the
//! same path it would have taken had the digest never failed.

use clap::{Args, Subcommand};

use crate::cli::CmdError;

mod clear;
mod control;
mod list;
mod remote;

use clear::clear;
use list::list;

pub(crate) use control::{canonical_control, compute_target, resolve_target};
pub(crate) use remote::{remote_read, remote_read_head, remote_read_tail};

#[derive(Subcommand)]
pub enum QuarantineCommands {
    /// List the digests this host will not roll out again.
    List(QuarantineListArgs),
    /// Retire exactly one quarantined digest so the agent retries it.
    Clear(QuarantineClearArgs),
}

#[derive(Args)]
pub struct QuarantineListArgs {
    pub product: String,
    /// Registry target name. Optional only while the product rolls out to one.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct QuarantineClearArgs {
    pub product: String,
    /// Registry target name. Never inferred: this command rewrites that host's
    /// rollout state, and the host is not a detail to guess at.
    #[arg(long)]
    target: String,
    /// The exact quarantined artifact digest, as `quarantine list` prints it.
    #[arg(long)]
    digest: String,
    /// Why this digest is being given another chance. Required, recorded in the
    /// audit trail beside the state file, and never defaulted.
    #[arg(long)]
    reason: String,
    #[arg(long)]
    json: bool,
}

/// Splice compile-time constants into a fixed remote program. Values are
/// shell-quoted by the caller; nothing operator-supplied reaches the shell
/// unquoted.
fn splice(template: &str, marks: &[(&str, &str)]) -> String {
    marks
        .iter()
        .fold(template.to_string(), |script, (mark, value)| {
            script.replace(mark, value)
        })
}

pub async fn dispatch(command: QuarantineCommands) -> Result<(), CmdError> {
    match command {
        QuarantineCommands::List(args) => list(&args).await,
        QuarantineCommands::Clear(args) => clear(&args).await,
    }
}
