//! A command's host, resolved through the registry-authorized host channel
//! and refused with the class of what was wrong: a name the registry does not
//! hold is `not_found`, a registry entry the channel cannot reach is
//! `refused`, and a registry nobody could read is `infra_down`.
//!
//! The host channel's [`DeployError`](crate::deploy::DeployError) carries only
//! a sentence, so every command that turned it into a `CmdError` printed
//! `the command failed and we could not attribute the failure`. The class is
//! decided here, by asking the registry the question the channel refused on,
//! never by reading the sentence.

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::primitives::failure::FailureCode;
use crate::targets::{ComputeTarget, Registry};

/// [`host_channel::resolve_target`], refused with its class.
pub fn resolved_host<'a>(
    registry: &'a Registry,
    target: &str,
) -> Result<&'a ComputeTarget, CmdError> {
    host_channel::resolve_target(registry, target).map_err(|error| {
        let code = if registry.lookup(target).is_none() {
            FailureCode::NotFound
        } else {
            FailureCode::Refused
        };
        CmdError::click(error.to_string()).stating(code)
    })
}

/// The canonical registry's entry for `target`, refused with its class: a
/// registry nobody could read is `infra_down`, and the entry itself is
/// judged by [`resolved_host`].
pub async fn canonical_host(target: &str) -> Result<ComputeTarget, CmdError> {
    let registry = host_channel::canonical_registry()
        .await
        .map_err(|error| CmdError::click(error.to_string()).stating(FailureCode::InfraDown))?;
    resolved_host(&registry, target).cloned()
}
