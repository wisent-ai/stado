//! What enrollment asks the machine before anything is written: its
//! hostname and release platform, through Stado's deploy channel.

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// Probe one fixed identity command through Stado's existing deploy channel.
/// A channel that does not open is infra_down; an answer that is not one
/// line is refused, because the machine is not what enrollment can verify.
async fn probe_identity_field(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
    command: &str,
) -> Result<String, CmdError> {
    let (argv, _key) = crate::cli::fleet::key::channel_argv(target, destination, command).await?;
    let output = runner(crate::deploy::CommandSpec::new(argv))
        .await
        .map_err(|exc| {
            CmdError::unreachable(format!("ssh to {destination} could not be started: {exc}"))
        })?;
    if !output.ok() {
        return Err(
            CmdError::click(format!("cannot verify {destination}: {}", output.detail()))
                .stating(FailureCode::InfraDown),
        );
    }
    let value = output.stdout.trim();
    if value.is_empty() || value.lines().count() != 1 {
        return Err(CmdError::refused(format!(
            "{destination} returned an invalid {command} value"
        )));
    }
    Ok(value.to_string())
}

/// Verify hostname and immutable-release platform before any registry write.
pub(super) async fn probe_identity(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
) -> Result<(String, &'static str), CmdError> {
    let raw_hostname = probe_identity_field(runner, target, destination, "hostname").await?;
    let hostname = crate::targets::normalize_hostname(&raw_hostname);
    if hostname.is_empty() {
        return Err(CmdError::refused(format!(
            "{destination} returned an empty hostname"
        )));
    }
    let os = probe_identity_field(runner, target, destination, "uname -s").await?;
    let arch = probe_identity_field(runner, target, destination, "uname -m").await?;
    let platform =
        crate::cli::fleet::enroll::release_platform(&os, &arch).map_err(CmdError::refused)?;
    Ok((hostname, platform))
}
