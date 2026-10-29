//! The fleet registry handed to an attached machine as its last-known-good
//! copy.
//!
//! An attached machine reaches the fleet store through its resolver, and its
//! resolver learns where the registry authority is from the registry copy
//! it keeps. A standalone Stado's copy is its own registry, which names no
//! authority and names the machine by its hostname, so after attaching its
//! `serve --auto` still resolved that name, refused the registered one and
//! never started the resolver that would have fetched the fleet's. The
//! machine's own `stado registry push - --last-known-good` takes the fleet
//! registry instead (`registry push` accepts it on a copy that names no
//! authority, when the document names another host as authority and lists
//! this one).

use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// Hand `document`, the registry this host read, to `target` as its copy.
pub(super) async fn hand_over(
    target: &ComputeTarget,
    document: &serde_json::Value,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    let home = crate::deploy::host_channel::remote_home(target, runner)
        .await
        .map_err(CmdError::from)?;
    let program = format!("{home}/.stado/bin/stado");
    let payload = serde_json::to_string(document)?;
    let output = crate::deploy::host_channel::run_program_with_stdin(
        target,
        &[
            program.as_str(),
            "registry",
            "push",
            "-",
            "--last-known-good",
        ],
        &payload,
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{}: its Stado refused the fleet registry as its last-known-good copy: {}",
            target.name,
            crate::deploy::host_channel::whole_failure(&output)
        ))
        .stating(FailureCode::Refused));
    }
    echo(&format!(
        "[attach] {}: {}",
        target.name,
        output.stdout.trim()
    ));
    Ok(())
}
