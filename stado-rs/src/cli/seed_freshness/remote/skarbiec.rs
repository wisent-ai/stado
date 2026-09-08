//! The vault's half: one Skarbiec read on the host, and the release-controlled
//! resolution of the Skarbiec binary that performs it.

use serde_json::Value;

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

/// Resolve Skarbiec through the host's release agent, using the same observed
/// active-binary contract consumed by Weles.
///
/// A product/target with no release-control policy may still use the historical
/// `$HOME/.stado/bin/skarbiec` install. Once the policy exists, however, desired
/// state and executable files are not evidence that a release is active: a
/// quarantined candidate leaves both behind. The host's installed Stado must
/// identify and validate the exact process, proxy target, manifest identity,
/// immutable directory and executable that are actually active.
async fn release_managed_skarbiec(
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    home: &str,
) -> Result<String, CmdError> {
    let legacy = format!("{home}/.stado/bin/skarbiec");
    let document = crate::cli::registry::fetch_document().await?;
    let control = crate::release_control::control(&document).map_err(CmdError::click)?;
    let Some(control) = control else {
        return Ok(legacy);
    };
    let Some(policy) = control.products.get("skarbiec") else {
        return Ok(legacy);
    };
    let Some(target) = policy.targets.get(resolved.name.as_str()) else {
        return Ok(legacy);
    };

    let stado = format!("{home}/.stado/bin/stado");
    let invocation = [
        stado.as_str(),
        "release",
        "active-binary",
        "skarbiec",
        "--target",
        resolved.name.as_str(),
        "--json",
    ];
    let output = crate::deploy::host_channel::run_program(resolved, &invocation, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: release-controlled Skarbiec has no available active binary: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "active release unavailable")
        )));
    }
    let active: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Stado active-binary returned unreadable JSON: {error}",
            resolved.name
        ))
    })?;
    let field = |name: &str| {
        active
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: Stado active-binary omitted {name:?}",
                    resolved.name
                ))
            })
    };
    let state = field("state")?;
    let product = field("product")?;
    let active_target = field("target")?;
    let version = field("version")?;
    let platform = field("platform")?;
    let artifact_sha256 = field("artifact_sha256")?;
    let manifest_sha256 = field("manifest_sha256")?;
    let path = field("path")?;
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    if state != "active"
        || product != "skarbiec"
        || active_target != resolved.name
        || platform != target.platform
        || !digest(artifact_sha256)
        || !digest(manifest_sha256)
    {
        return Err(CmdError::click(format!(
            "{}: Stado active-binary returned an invalid Skarbiec identity",
            resolved.name
        )));
    }
    let expected = crate::release_control::release_directory(policy, target, version, platform)
        .join(&policy.binary);
    if !std::path::Path::new(path).is_absolute() || std::path::Path::new(path) != expected {
        return Err(CmdError::click(format!(
            "{}: Stado active-binary returned path {path:?}, expected {}",
            resolved.name,
            expected.display()
        )));
    }
    Ok(path.to_string())
}
