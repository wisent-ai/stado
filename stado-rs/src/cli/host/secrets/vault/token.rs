use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::secrets::vault::vault_word;

/// How `vault_token_sync` delivers: verify only or install, and whether the
/// destination reads the source's vault through its resolver route instead
/// of holding a copy.
#[derive(Clone, Copy)]
pub enum TokenSyncMode {
    Install,
    Check,
    InstallShared,
    CheckShared,
}

impl TokenSyncMode {
    pub fn from_flags(check: bool, shared_vault: bool) -> Self {
        match (check, shared_vault) {
            (true, true) => Self::CheckShared,
            (true, false) => Self::Check,
            (false, true) => Self::InstallShared,
            (false, false) => Self::Install,
        }
    }

    fn shared_vault(self) -> bool {
        matches!(self, Self::InstallShared | Self::CheckShared)
    }

    /// The mode word the host-side payload reads.
    fn payload_word(self) -> &'static str {
        match self {
            Self::CheckShared => "check-shared",
            Self::Check => "check",
            Self::InstallShared => "install-shared",
            Self::Install => "install",
        }
    }
}

/// The vault argument the shared install payload receives and ignores.
const SHARED_VAULT_UNUSED: &str = "-";

/// Run the host's installed Stado custody primitive with the operation,
/// vault, consumer and token file as `$1`–`$4`; stdin passes through.
const HOST_CUSTODY: &str = r#"stado="$HOME/.stado/bin/stado"
[ -x "$stado" ] || stado="$(command -v stado)"
exec "$stado" credentials token custody-local "$@""#;

/// The destination of one token delivery: its host, and the vault the
/// payload checks the grant in (unused for a shared-vault destination).
struct SharedDestination {
    target: crate::targets::ComputeTarget,
    vault: String,
}

/// Deliver a bootstrap bearer without minting, renewing, or widening a grant.
/// Both declared vaults must already hold the same owner and complete grant.
pub async fn vault_token_sync(
    from_host: &str,
    target: &str,
    consumer: &str,
    source_token_file: &str,
    token_file: &str,
    mode: TokenSyncMode,
    json_output: bool,
) -> Result<(), CmdError> {
    use crate::cli::host::machine::users::credentials::credential_host;
    use crate::deploy::host_channel;
    use crate::primitives::failure::FailureCode;

    vault_word("consumer", consumer).map_err(|error| error.machine_readable(json_output))?;
    if source_token_file.trim().is_empty() || token_file.trim().is_empty() {
        return Err(
            CmdError::usage("source and destination token files must be named")
                .machine_readable(json_output),
        );
    }
    // Resolve both declarations before reading any bearer. The payload only
    // crosses encrypted host channels in memory; it is never a CLI argument.
    let source = credential_host(from_host)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    // A shared-vault destination reads the owner through its resolver route
    // and holds no vault of its own; the payload verifies its bearer against
    // the owner's grant and never opens a destination vault. Requiring one
    // would refuse `--shared-vault` onto a host whose local copy was retired
    // as `stado credentials vault` directs, with `declares no vault
    // authority`, so a re-minted owner bearer could never reach it.
    let destination = if mode.shared_vault() {
        let target = crate::deploy::host_channel::canonical_target(target)
            .await
            .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
        SharedDestination {
            target,
            vault: SHARED_VAULT_UNUSED.to_string(),
        }
    } else {
        let host = credential_host(target)
            .await
            .map_err(|error| error.machine_readable(json_output))?;
        SharedDestination {
            target: host.target,
            vault: host.vault,
        }
    };
    let runner = crate::deploy::production_runner();
    // `--shared-vault` names a destination that reads the source's vault
    // through its own resolver route instead of a copy. The registry decides
    // whether that is true: the destination declares a resolver adapter for
    // Skarbiec, and the service directory places Skarbiec on the source.
    if mode.shared_vault() {
        let registry = crate::targets::load_registry_auto()
            .await
            .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
        let document = serde_json::to_value(&registry)
            .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
        let document = &document;
        let routed = document
            .pointer("/service_directory/services/skarbiec/active_host")
            .and_then(Value::as_str)
            == Some(source.target.name.as_str());
        let adapter = document
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|host| {
                host.get("name").and_then(Value::as_str) == Some(destination.target.name.as_str())
            })
            .and_then(|host| host.pointer("/service_resolver/adapters"))
            .and_then(Value::as_array)
            .is_some_and(|adapters| {
                adapters.iter().any(|adapter| {
                    adapter.get("service").and_then(Value::as_str) == Some("skarbiec")
                })
            });
        if !routed || !adapter {
            return Err(CmdError::click(format!(
                "{}: --shared-vault needs the service directory to place skarbiec on {} and {} to declare a resolver adapter for skarbiec (placed there: {routed}, adapter: {adapter})",
                destination.target.name, source.target.name, destination.target.name
            ))
            .stating(FailureCode::Refused)
            .machine_readable(json_output));
        }
    }
    let exported = host_channel::run_program(
        &source.target,
        &[
            "/bin/sh",
            "-c",
            HOST_CUSTODY,
            "stado-token-custody",
            "export",
            &source.vault,
            consumer,
            source_token_file,
        ],
        &runner,
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "{}: token export failed: {error}",
            source.target.name
        ))
        .stating(FailureCode::InfraDown)
        .machine_readable(json_output)
    })?;
    if !exported.ok() {
        return Err(CmdError::click(format!(
            "{}: token export refused: {}",
            source.target.name,
            host_channel::last_error_line(&exported, "host token export failed")
        ))
        .stating(FailureCode::Refused)
        .machine_readable(json_output));
    }
    let installed = host_channel::run_program_with_stdin(
        &destination.target,
        &[
            "/bin/sh",
            "-c",
            HOST_CUSTODY,
            "stado-token-custody",
            mode.payload_word(),
            &destination.vault,
            consumer,
            token_file,
        ],
        &exported.stdout,
        &runner,
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "{}: token delivery failed: {error}",
            destination.target.name
        ))
        .stating(FailureCode::InfraDown)
        .machine_readable(json_output)
    })?;
    drop(exported);
    if !installed.ok() {
        return Err(CmdError::click(format!(
            "{}: token delivery refused: {}",
            destination.target.name,
            host_channel::last_error_line(&installed, "host token delivery failed")
        ))
        .stating(FailureCode::Refused)
        .machine_readable(json_output));
    }
    let mut report: Value = serde_json::from_str(installed.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "token delivery returned unreadable metadata: {error}"
        ))
        .stating(FailureCode::InfraDown)
        .machine_readable(json_output)
    })?;
    report["target"] = json!(destination.target.name);
    report["source_host"] = json!(source.target.name);
    let status = report["status"]
        .as_str()
        .filter(|status| {
            matches!(
                *status,
                "token_synced" | "token_unchanged" | "token_checked"
            )
        })
        .ok_or_else(|| {
            CmdError::click("token delivery returned no recognized outcome")
                .stating(FailureCode::InfraDown)
                .machine_readable(json_output)
        })?;
    let delivered_path = report["skarbiec"]["token_file"]
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            CmdError::click("token delivery returned no verified file")
                .stating(FailureCode::InfraDown)
                .machine_readable(json_output)
        })?;
    if report["skarbiec"]["ok"] != true || report["skarbiec"]["consumer"] != consumer {
        return Err(
            CmdError::click("token delivery did not verify the requested consumer")
                .stating(FailureCode::InfraDown)
                .machine_readable(json_output),
        );
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: {} for {consumer}; source {}; grants unchanged",
            destination.target.name, status, source.target.name,
        );
        println!("Bearer file: {delivered_path}");
    }
    Ok(())
}
