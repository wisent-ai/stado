use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::{ComputeTarget, Registry};

use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};

pub(crate) struct CredentialHost {
    pub target: ComputeTarget,
    pub home: String,
    pub vault: String,
    pub gnupg_home: String,
}

/// Resolve credential custody from the host's own durable Stado declaration.
///
/// The remote configuration document is the declaration every service on that
/// host consumes. An absent field is never replaced with a conventional path:
/// a plausible default is precisely how two vaults can both receive real writes.
pub(crate) async fn credential_host(target: &str) -> Result<CredentialHost, CmdError> {
    let target = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&target, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let configuration = remote_config_output(&target, RemoteConfigAction::Show, &runner).await?;
    let document: Value = serde_json::from_str(&configuration).map_err(|error| {
        CmdError::click(format!(
            "{}: the declared Stado configuration could not be read: {error}",
            target.name
        ))
    })?;
    let declared = document
        .pointer("/resolved/skarbiec_vault_file")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no vault authority; add it to secrets.skarbiec.vault_file",
                target.name
            ))
        })?;
    let vault = declared
        .strip_prefix("$HOME/")
        .map(|tail| format!("{home}/{tail}"))
        .unwrap_or_else(|| declared.to_string());
    let environment = crate::deploy::host_channel::run_command(
        &target,
        "printf '%s\n' \"${GNUPGHOME:-$HOME/.gnupg}\"",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: GNUPGHOME could not be resolved from the host environment",
            target.name
        )));
    }
    let gnupg_home = environment.stdout.trim().to_string();
    if gnupg_home.is_empty() {
        return Err(CmdError::click(format!(
            "{}: GNUPGHOME is empty; declare it in the host environment",
            target.name
        )));
    }
    Ok(CredentialHost {
        target,
        home,
        vault,
        gnupg_home,
    })
}

/// The registry for `--registry-source` (Python `load_targets(source=...)`:
/// "gcs" = the canonical remote registry only (whichever store
/// `WC_STORAGE_BACKEND` selects), "local" = bundled file, "auto" = remote
/// with bundled fallback).
pub(super) async fn load_registry_by_source(source: &str) -> Result<Registry, CmdError> {
    match source {
        "gcs" => crate::targets::fetch_registry_remote()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
        "local" => {
            crate::targets::load_bundled_registry().map_err(|exc| CmdError::click(exc.to_string()))
        }
        _ => crate::targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
    }
}
