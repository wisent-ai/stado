//! An enrolled, bootstrapped machine joins Stado's own network before
//! anything else is declared for it: the Stado it just received runs
//! `stado network up` on the machine itself, which makes the network the
//! machine's preferred registry path and proves its own SSH server answers
//! through it.

use serde_json::Value;

use crate::cli::CmdError;

/// The installed binary relative to the login directory SSH starts in, the
/// place `bootstrap` installs it.
const NETWORK_UP: &[&str] = &[".stado/bin/stado", "network", "up", "--json"];

/// Run `network up` on enrolled machine `name` and return its receipt.
pub(super) async fn join(name: &str) -> Result<Value, CmdError> {
    let target = crate::deploy::host_channel::canonical_target(name).await?;
    let output = crate::deploy::host_channel::run_program(
        &target,
        NETWORK_UP,
        &crate::deploy::production_runner(),
    )
    .await?;
    if !output.succeeded() {
        return Err(CmdError::unreachable(format!(
            "'{name}' is registered and bootstrapped but did not join the Stado network: {}; `stado network up` on it retries",
            output.detail().trim()
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::unreachable(format!(
            "'{name}' ran `stado network up` but its receipt is not JSON ({error}): {}",
            output.stdout.trim()
        ))
    })
}
