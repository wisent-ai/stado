use serde_json::{json, Value};

use crate::cli::host::secrets::vault::vault_word;
use crate::cli::CmdError;

/// What a token delivery does at its destination: verify only or install,
/// and whether the destination reads the source's vault through its
/// resolver route instead of holding a copy.
#[derive(Clone, Copy)]
pub struct Delivery {
    pub verify_only: bool,
    pub shared_vault: bool,
}

impl Delivery {
    /// Write the bearer, checked against the destination's own grant.
    pub const INSTALL: Self = Self {
        verify_only: false,
        shared_vault: false,
    };

    /// The custody operation the destination's Stado runs for it.
    fn operation(self) -> CustodyOperation {
        match (self.verify_only, self.shared_vault) {
            (true, true) => CustodyOperation::CheckShared,
            (true, false) => CustodyOperation::Check,
            (false, true) => CustodyOperation::InstallShared,
            (false, false) => CustodyOperation::Install,
        }
    }
}

/// One step of `stado credentials token custody`, spelled on the
/// command line by its clap name: `export` at the source, a delivery at the
/// destination.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum CustodyOperation {
    Export,
    Install,
    Check,
    InstallShared,
    CheckShared,
}

impl CustodyOperation {
    /// The delivery this operation performs; `export` delivers nothing.
    pub fn delivery(self) -> Option<Delivery> {
        let (verify_only, shared_vault) = match self {
            Self::Export => return None,
            Self::Install => (false, false),
            Self::Check => (true, false),
            Self::InstallShared => (false, true),
            Self::CheckShared => (true, true),
        };
        Some(Delivery {
            verify_only,
            shared_vault,
        })
    }

    /// How the host's Stado is asked for it.
    fn name(self) -> Result<String, CmdError> {
        use clap::ValueEnum;
        let value = self.to_possible_value().ok_or_else(|| {
            CmdError::click("a custody operation is hidden from the command line".to_string())
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        Ok(value.get_name().to_owned())
    }
}

/// The vault argument the shared install payload receives and ignores.
const SHARED_VAULT_UNUSED: &str = "-";

/// Run the host's installed Stado custody primitive with the operation,
/// vault, consumer and token file as `$1`–`$4`; stdin passes through.
const HOST_CUSTODY: &str = r#"stado="$HOME/.stado/bin/stado"
[ -x "$stado" ] || stado="$(command -v stado)"
exec "$stado" credentials token custody "$@""#;

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
    mode: Delivery,
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
    // as `stado credentials vault show` directs, with `declares no vault
    // authority`, so a re-minted owner bearer could never reach it.
    let destination = if mode.shared_vault {
        let target = crate::cli::canonical_host(target)
            .await
            .map_err(|error| error.machine_readable(json_output))?;
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
    if mode.shared_vault {
        let registry = crate::targets::load_registry_auto()
            .await
            .map_err(|error| CmdError::from(error).machine_readable(json_output))?;
        let document = serde_json::to_value(&registry)
            .map_err(|error| CmdError::from(error).machine_readable(json_output))?;
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
    let export_operation = CustodyOperation::Export
        .name()
        .map_err(|error| error.machine_readable(json_output))?;
    let exported = host_channel::run_program(
        &source.target,
        &[
            "/bin/sh",
            "-c",
            HOST_CUSTODY,
            "stado-token-custody",
            &export_operation,
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
    if !exported.succeeded() {
        return Err(CmdError::click(format!(
            "{}: credentials token custody {export_operation} refused for {consumer} at {source_token_file} (exit {}); stderr:\n{}",
            source.target.name,
            exported.code,
            exported.stderr.trim()
        ))
        .stating(FailureCode::Refused)
        .machine_readable(json_output));
    }
    let delivery_operation = mode
        .operation()
        .name()
        .map_err(|error| error.machine_readable(json_output))?;
    let installed = host_channel::run_program_with_stdin(
        &destination.target,
        &[
            "/bin/sh",
            "-c",
            HOST_CUSTODY,
            "stado-token-custody",
            &delivery_operation,
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
    if !installed.succeeded() {
        return Err(CmdError::click(format!(
            "{}: credentials token custody {delivery_operation} refused for {consumer} at {token_file} (exit {}); stderr:\n{}",
            destination.target.name,
            installed.code,
            installed.stderr.trim()
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
        .map(str::to_owned)
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
    // A check changes nothing, the registry included; a delivery is what the
    // destination now holds, so the registry says so.
    if !mode.verify_only {
        record_held_bearer(&destination.target.name, consumer, &delivered_path)
            .await
            .map_err(|error| error.machine_readable(json_output))?;
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

/// Record in the registry that `host` holds `consumer`'s bearer at `file`
/// (`targets[].bearers`), so `stado registry host show` says which of the
/// fleet's credentials sit on that host.
pub(crate) async fn record_held_bearer(
    host: &str,
    consumer: &str,
    file: &str,
) -> Result<(), CmdError> {
    let bearer = crate::targets::HeldBearer {
        consumer: consumer.to_string(),
        file: file.to_string(),
    };
    crate::cli::registry::commit_document(|document| {
        crate::targets::with_held_bearer(document, host, &bearer).map_err(|cause| {
            CmdError::click(cause).stating(crate::primitives::failure::FailureCode::Config)
        })
    })
    .await
    .map(drop)
    .map_err(|error| {
        error.within(format!(
            "{host} holds the bearer of {consumer} at {file}, but the registry does not record \
             it; run the same command again to record it"
        ))
    })
}

/// Removes one bearer file on the host it runs on, given its absolute path as
/// `$1`: refused for a symbolic link, for anything but a regular file this
/// account owns, and for a file whose directory resolves outside the
/// account's home; a file already gone is said as `absent`.
const REMOVE_BEARER: &str = r#"set -u
file=$1
if [ -L "$file" ]; then printf '%s is a symbolic link, not a bearer file; nothing was removed\n' "$file" >&2; exit 3; fi
if [ ! -e "$file" ]; then printf 'absent\n'; exit 0; fi
if [ ! -f "$file" ] || [ ! -O "$file" ]; then printf '%s is not a regular file this account owns; nothing was removed\n' "$file" >&2; exit 3; fi
home=$(cd "$HOME" && pwd -P) || exit
directory=$(cd "${file%/*}" && pwd -P) || exit
case "$directory/" in "$home"/*) ;; *) printf '%s resolves to %s, outside the account home %s; nothing was removed\n' "$file" "$directory" "$home" >&2; exit 3;; esac
rm -f -- "$file" || exit
if [ -e "$file" ]; then printf '%s is still there after its removal\n' "$file" >&2; exit 1; fi
printf 'removed\n'"#;

/// The absolute path of a bearer file named the way `sync` names one: a bare
/// name under `~/.stado`, a `~/` or `$HOME/` path, or an absolute path inside
/// the account's `home`.
fn bearer_path(home: &str, value: &str) -> Result<String, CmdError> {
    let home = home.trim_end_matches('/');
    let below_home = match ["~/", "$HOME/"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
    {
        Some(rest) => rest.to_string(),
        None if value.starts_with('/') => value
            .strip_prefix(&format!("{home}/"))
            .ok_or_else(|| {
                CmdError::usage(format!(
                    "token file {value} is outside the host account's home {home}; Stado places \
                     bearer files only inside it"
                ))
            })?
            .to_string(),
        None if !value.contains('/') => format!(".stado/{value}"),
        None => {
            return Err(CmdError::usage(format!(
                "token file {value} must be a bare name under ~/.stado, a ~/ path or an absolute \
                 path inside the host account's home"
            )))
        }
    };
    if below_home
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(CmdError::usage(format!(
            "token file {value} names an empty, `.` or `..` path component"
        )));
    }
    Ok(format!("{home}/{below_home}"))
}

/// Take a bearer file off a host once the grant it opened is gone, and take
/// its record out of the registry (`targets[].bearers`).
///
/// The counterpart of `sync`, which puts one there. A bearer that still opens
/// a grant is a credential in use, so the file stays while the vault owner
/// records any grant for CONSUMER, while the registry records the file as
/// another consumer's, and while the host's own configuration names it as a
/// bearer file it reads. A file already absent is the effect asked for.
pub async fn vault_token_remove(
    target: &str,
    consumer: &str,
    token_file: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};
    use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
    use crate::deploy::host_channel;
    use crate::primitives::failure::FailureCode;

    let readable = |error: CmdError| error.machine_readable(json_output);
    vault_word("consumer", consumer).map_err(readable)?;
    if token_file.trim().is_empty() {
        return Err(readable(CmdError::usage("the token file must be named")));
    }
    let target = crate::cli::canonical_host(target).await.map_err(readable)?;
    let owner = crate::cli::directory::active_host("skarbiec")
        .await
        .map_err(readable)?
        .ok_or_else(|| {
            readable(
                CmdError::click(format!(
                    "the service directory places skarbiec on no host, so whether a grant for \
                     {consumer} still stands cannot be read; declare skarbiec's active host first"
                ))
                .stating(FailureCode::Config),
            )
        })?;
    let (owner_target, listing) =
        remote_skarbiec_json(&owner, &[String::from("grant"), String::from("list")])
            .await
            .map_err(readable)?;
    let grants = listing.as_array().ok_or_else(|| {
        readable(
            CmdError::click(format!(
                "{}: Skarbiec did not answer its grant list as an array",
                owner_target.name
            ))
            .stating(FailureCode::InfraDown),
        )
    })?;
    if grants
        .iter()
        .any(|grant| grant.get("consumer").and_then(Value::as_str) == Some(consumer))
    {
        return Err(readable(CmdError::refused(format!(
            "{}'s vault still records a grant for {consumer}, so its bearer on {} is a credential \
             in use; revoke the grant first (`stado credentials grant revoke {consumer} --host \
             {}`), then remove the file",
            owner_target.name, target.name, owner_target.name
        ))));
    }
    let runner = crate::deploy::production_runner();
    let home = host_channel::remote_home(&target, &runner)
        .await
        .map_err(|error| readable(CmdError::from(error)))?;
    let path = bearer_path(&home, token_file).map_err(readable)?;
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| readable(CmdError::from(error)))?;
    if let Some(held) = registry
        .targets
        .iter()
        .find(|entry| entry.name == target.name)
        .and_then(|entry| entry.extra.get(crate::targets::BEARERS_KEY))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|held| held.get("file").and_then(Value::as_str) == Some(path.as_str()))
    {
        if held.get("consumer").and_then(Value::as_str) != Some(consumer) {
            return Err(readable(CmdError::refused(format!(
                "the registry records {path} on {} as the bearer of {}, not of {consumer}; \
                 nothing was removed",
                target.name,
                crate::cli::reporting::field(&held["consumer"])
            ))));
        }
    }
    let configuration = remote_config_output(&target, RemoteConfigAction::Show, &runner)
        .await
        .map_err(readable)?;
    let configuration: Value = serde_json::from_str(&configuration).map_err(|error| {
        readable(
            CmdError::click(format!(
                "{} answered its configuration read with something that is not JSON: {error}",
                target.name
            ))
            .stating(FailureCode::InfraDown),
        )
    })?;
    if let Some((key, _)) = configuration["resolved"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(key, value)| key.ends_with("token_file") && value.as_str() == Some(path.as_str()))
    {
        return Err(readable(CmdError::refused(format!(
            "{}'s configuration reads its {key} bearer from {path}; nothing was removed",
            target.name
        ))));
    }
    let removed = host_channel::run_program(
        &target,
        &["/bin/sh", "-c", REMOVE_BEARER, "stado-token-remove", &path],
        &runner,
    )
    .await
    .map_err(|error| {
        readable(
            CmdError::click(format!("{}: bearer removal failed: {error}", target.name))
                .stating(FailureCode::InfraDown),
        )
    })?;
    if !removed.succeeded() {
        return Err(readable(
            CmdError::click(format!(
                "{}: {path} was not removed (exit {}): {}",
                target.name,
                removed.code,
                removed.stderr.trim()
            ))
            .stating(FailureCode::Refused),
        ));
    }
    let status = match removed.stdout.trim() {
        "removed" => "token_removed",
        "absent" => "token_absent",
        other => {
            return Err(readable(
                CmdError::click(format!(
                    "{}: the bearer removal answered {other:?}, neither removed nor absent",
                    target.name
                ))
                .stating(FailureCode::InfraDown),
            ))
        }
    };
    crate::cli::registry::commit_document(|document| {
        crate::targets::without_held_bearer(document, &target.name, &path)
            .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))
    })
    .await
    .map_err(|error| {
        readable(error.within(format!(
            "{path} is off {}, but the registry still records it; run the same command again \
             to take the record away",
            target.name
        )))
    })?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target.name,
                "consumer": consumer,
                "token_file": path,
                "status": status,
                "grant_owner": owner_target.name,
            }))?
        );
    } else if status == "token_removed" {
        println!(
            "{}: removed {path}, the bearer of {consumer}, whose grant {} no longer records",
            target.name, owner_target.name
        );
    } else {
        println!("{}: {path} is already absent", target.name);
    }
    Ok(())
}
