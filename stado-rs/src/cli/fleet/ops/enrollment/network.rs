//! An enrolled, bootstrapped machine joins Stado's own network before
//! anything else is declared for it: the Stado it just received runs
//! `stado network up` on the machine itself, which makes the network the
//! machine's preferred registry path and proves its own SSH server answers
//! through it.

use serde_json::Value;

use crate::cli::fleet::enroll::failures::{NETWORK_JOIN_FAILED, NETWORK_RECEIPT_UNREADABLE};
use crate::cli::CmdError;

/// The installed binary relative to the login directory SSH starts in, the
/// place `bootstrap` installs it.
const NETWORK_UP: &[&str] = &[".stado/bin/stado", "network", "up", "--json"];

/// Run `network up` on enrolled machine `name`, reached at `ssh`, and return
/// its receipt. The registration and the install stand whatever this
/// answers, and the refusal says so.
pub(super) async fn join(name: &str, ssh: &str) -> Result<Value, CmdError> {
    let failed = |answer: String| {
        CmdError::declared(
            &NETWORK_JOIN_FAILED,
            &[("name", name), ("ssh", ssh), ("answer", &answer)],
        )
    };
    let target = crate::deploy::host_channel::canonical_target(name)
        .await
        .map_err(|error| failed(error.to_string()))?;
    let output = crate::deploy::host_channel::run_program(
        &target,
        NETWORK_UP,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| failed(error.to_string()))?;
    if !output.succeeded() {
        return Err(failed(output.detail().trim().to_string()));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::declared(
            &NETWORK_RECEIPT_UNREADABLE,
            &[
                ("name", name),
                ("answer", &format!("{error}: {}", output.stdout.trim())),
            ],
        )
    })
}
