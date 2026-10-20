//! Changing an item the vault already holds: delete it, give it a new id,
//! replace its tags, or return it to the owner's control.

pub(in crate::cli::host) mod delete;
pub(in crate::cli::host) mod reclaim;
pub(in crate::cli::host) mod rename;
pub(in crate::cli::host) mod retag;

use crate::cli::CmdError;
use crate::deploy::host_channel::{run_command, whole_failure};
use crate::deploy::{shlex_quote, Runner};
use crate::targets::ComputeTarget;

/// Whether the Skarbiec build at `skarbiec` on `target` carries the command
/// whose usage line is `usage`. The usage literal, never the bare command
/// name, is the discriminator: rustc packs string literals into one
/// unterminated blob, so a binary that carries the command shows
/// `...setgetretagdelete...` on a single line and a whole-line match for the
/// name reports absent on a build that has it. A binary `strings` cannot read
/// (it is not there, or the host has no `strings`) is refused with the words
/// `strings` gave, never read as a build that predates the command.
pub(super) async fn skarbiec_carries(
    target: &ComputeTarget,
    runner: &Runner,
    skarbiec: &str,
    usage: &str,
) -> Result<bool, CmdError> {
    let probe = run_command(
        target,
        &format!(
            "if words=$(strings -a {binary}); then \
             printf '%s\\n' \"$words\" | grep -q -F {usage} && echo carries || echo lacks; \
             else false; fi",
            binary = shlex_quote(skarbiec),
            usage = shlex_quote(usage),
        ),
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    if !probe.succeeded() {
        return Err(CmdError::click(format!(
            "{}: the Skarbiec build at {skarbiec} could not be read for its commands: {}",
            target.name,
            whole_failure(&probe)
        )));
    }
    match probe.stdout.trim() {
        "carries" => Ok(true),
        "lacks" => Ok(false),
        other => Err(CmdError::click(format!(
            "{}: reading the commands of the Skarbiec build at {skarbiec} answered {other:?}, \
             neither carries nor lacks",
            target.name
        ))),
    }
}
