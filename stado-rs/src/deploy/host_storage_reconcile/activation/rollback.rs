use super::*;

const ROLLBACK_OBJECT_API_SCRIPT: &str = r#"set -euo pipefail
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'unsupported_os\n' >&2
  exit 65
fi
label=com.wisent.always-on.stado-object-api
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
store=@PRIMARY@
backup_backend=@BACKUP_BACKEND@
backup_store=@BACKUP@
config=@CONFIG@
port=@PORT@
log="$HOME/.stado/logs/$label.log"
work="$HOME/.stado/work/object-api-recovery"
[ -x "$program" ] && [ -d "$store" ] && [ -r "$store/registry.json" ]
if [ -n "$backup_store" ]; then
  [ "$backup_backend" = local ] && [ -d "$backup_store" ] && [ -r "$backup_store/registry.json" ]
fi
/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.captured-prior.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)
/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$backup_backend" "$backup_store" "$account" "$log" "$HOME" "$config" "$port" <<'PY'
import plistlib, sys
path, label, program, store, backup_backend, backup_store, account, log, home, config, port = sys.argv[1:]
environment = {
    "HOME": home,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "STADO_CONFIG": config,
    "GNUPGHOME": f"{home}/.gnupg",
    "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
    "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
    "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
    "WC_STORAGE_BACKEND": "local",
    "WC_LOCAL_STORAGE_PATH": store,
}
if backup_store:
    environment["WC_BACKUP_STORAGE_BACKEND"] = backup_backend
    environment["WC_BACKUP_LOCAL_STORAGE_PATH"] = backup_store
document = {
    "Label": label,
    "ProgramArguments": [program, "dashboard", "--bind", "127.0.0.1", "--port", port],
    "EnvironmentVariables": environment,
    "RunAtLoad": True,
    "KeepAlive": True,
    "UserName": account,
    "StandardOutPath": log,
    "StandardErrorPath": log,
}
with open(path, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
PY
/usr/bin/plutil -lint "$staged" >/dev/null
/usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl enable "system/$label"
/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
printf 'STADO_OBJECT_API_ROUTE\tcaptured-prior\n'
"#;

pub(in crate::deploy::host_storage_reconcile) fn restore_priority(role: &str) -> u8 {
    match role {
        "object-api" => 0,
        "coordinator" => 1,
        "agent" | "disk-cleanup" => 2,
        "release-agent" => 3,
        "runner" => 4,
        "current-runner" => 5,
        _ => 2,
    }
}

pub(in crate::deploy::host_storage_reconcile) fn managed_writer(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
) -> service::ManagedService {
    let kind = if writer.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    managed_from_unit(target, &writer.label, &writer.path, kind)
}

pub(in crate::deploy::host_storage_reconcile) fn object_recovery_script(
    writer: &WriterFence,
    primary: &str,
    backup: Option<&str>,
) -> Result<PreparedScript, DeployError> {
    if primary.is_empty() || backup == Some("") {
        return Err(DeployError(
            "prepared object recovery contains an empty physical root".to_string(),
        ));
    }
    let config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            DeployError(
                "object API has neither observed nor declared STADO_CONFIG for recovery"
                    .to_string(),
            )
        })?;
    let port = writer
        .listener_port
        .ok_or_else(|| DeployError("captured object API port is absent".to_string()))?;
    let body = ROLLBACK_OBJECT_API_SCRIPT
        .replace("@PRIMARY@", &shlex_quote(primary))
        .replace(
            "@BACKUP_BACKEND@",
            &shlex_quote(if backup.is_some() { "local" } else { "" }),
        )
        .replace("@BACKUP@", &shlex_quote(backup.unwrap_or("")))
        .replace("@CONFIG@", &shlex_quote(config))
        .replace("@PORT@", &port.to_string());
    Ok(prepared_script(body))
}

pub(in crate::deploy::host_storage_reconcile) fn recovered_object_store(
    fence: &LifecycleFence,
) -> Result<crate::queue::JobStorage, DeployError> {
    let endpoint = fence
        .writers
        .iter()
        .find(|writer| writer.role == "object-api")
        .and_then(|writer| writer.restored_route.as_ref())
        .and_then(|proof| proof.get("served_store"))
        .and_then(|proof| proof.get("endpoint"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeployError("recovered object API proof omitted its endpoint".to_string())
        })?;
    let backend = crate::queue::StadoObjectBackend::new(
        endpoint,
        "probierz",
        "~/.stado/queue-object-api-token",
        "",
    )
    .map_err(|error| {
        DeployError(format!(
            "cannot bind typed observation to recovered object API: {error}"
        ))
    })?;
    Ok(crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "recovered-stado-object",
    ))
}
