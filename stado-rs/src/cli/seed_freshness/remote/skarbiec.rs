//! The vault's half: one Skarbiec read on the host, and the release-controlled
//! resolution of the Skarbiec binary that performs it.

use serde_json::Value;

use crate::cli::host::release_managed_skarbiec;
use crate::cli::CmdError;

/// Run one Skarbiec read on the host and parse its JSON answer.
///
/// The vault and GnuPG paths are resolved by the target itself, and arguments
/// stay separate all the way through the host channel, so nothing an operator
/// typed enters a remote shell command. Modelled on `cli::host`'s own Skarbiec
/// reads; kept local because this diagnostic needs exactly one command and no
/// write path.
pub(in crate::cli::seed_freshness) async fn remote_seed_state(
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    home: &str,
    vault: &str,
    gnupg_home: &str,
    arguments: &[String],
) -> Result<Value, CmdError> {
    let skarbiec = release_managed_skarbiec(resolved, runner, home).await?;
    let tool_path = format!(
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/local/MacGPG2/bin:{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let mut invocation = vec![
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
    ];
    invocation.extend(arguments.iter().map(String::as_str));
    let output = crate::deploy::host_channel::run_program(resolved, &invocation, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec totp-seed-state failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec totp-seed-state returned unreadable JSON: {error}",
            resolved.name
        ))
    })
}
