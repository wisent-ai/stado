use super::*;

const ROLLBACK_OBJECT_API_SCRIPT: &str = r#"set -euo pipefail
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'unsupported_os\n' >&2
  exit 65
fi
label=@LABEL@
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
store=@PRIMARY@
backup_backend=@BACKUP_BACKEND@
backup_store=@BACKUP@
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
printf '%s' @PLIST@ | /usr/bin/base64 -D > "$staged"
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
        "disk-cleanup" => 1,
        "runner" => 3,
        "current-runner" => 4,
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
    // The unit restored is the one the fence captured writing, under its own
    // label, whichever label the host's Stado process ran under then.
    let unit = object_api_unit(&writer.label, primary, backup, config, port)?;
    let body = ROLLBACK_OBJECT_API_SCRIPT
        .replace("@LABEL@", &shlex_quote(&writer.label))
        .replace("@PRIMARY@", &shlex_quote(primary))
        .replace(
            "@BACKUP_BACKEND@",
            &shlex_quote(if backup.is_some() { "local" } else { "" }),
        )
        .replace("@BACKUP@", &shlex_quote(backup.unwrap_or("")))
        .replace("@PLIST@", &shlex_quote(&unit))
        .replace("@PORT@", &port.to_string());
    Ok(prepared_script(body))
}

/// The object API's launchd definition as the transaction restores it,
/// base64-encoded XML: the worker renders it on the host it runs on, for the
/// account it runs as.
fn object_api_unit(
    label: &str,
    primary: &str,
    backup: Option<&str>,
    config: &str,
    port: u16,
) -> Result<String, DeployError> {
    use plist::{Dictionary, Value as Plist};
    let home = std::env::var("HOME").map_err(|_| DeployError("HOME is not set".to_string()))?;
    let account = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
        .ok_or_else(|| DeployError("the managed account has no user name".to_string()))?;
    let log = format!("{home}/.stado/logs/{label}.log");
    let text = |value: &str| Plist::String(value.to_string());
    let mut environment = Dictionary::new();
    for (name, value) in [
        ("HOME", home.clone()),
        (
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        ),
        ("STADO_CONFIG", config.to_string()),
        ("GNUPGHOME", format!("{home}/.gnupg")),
        (
            "SKARBIEC_VAULT_FILE",
            format!("{home}/.stado/skarbiec.vault.json"),
        ),
        ("WC_SKARBIEC_CONSUMER", "stado".to_string()),
        (
            "WC_SKARBIEC_TOKEN_FILE",
            format!("{home}/.stado/stado-skarbiec-token"),
        ),
        ("WC_STORAGE_BACKEND", "local".to_string()),
        ("WC_LOCAL_STORAGE_PATH", primary.to_string()),
    ] {
        environment.insert(name.to_string(), Plist::String(value));
    }
    if let Some(backup) = backup {
        environment.insert("WC_BACKUP_STORAGE_BACKEND".to_string(), text("local"));
        environment.insert("WC_BACKUP_LOCAL_STORAGE_PATH".to_string(), text(backup));
    }
    let program = format!("{home}/.stado/bin/stado");
    let port = port.to_string();
    let arguments = [
        program.as_str(),
        "serve",
        "--api",
        "--bind",
        "127.0.0.1",
        "--port",
        port.as_str(),
    ];
    let mut unit = Dictionary::new();
    unit.insert("Label".to_string(), text(label));
    unit.insert(
        "ProgramArguments".to_string(),
        Plist::Array(arguments.iter().map(|argument| text(argument)).collect()),
    );
    unit.insert(
        "EnvironmentVariables".to_string(),
        Plist::Dictionary(environment),
    );
    unit.insert("RunAtLoad".to_string(), Plist::Boolean(true));
    unit.insert("KeepAlive".to_string(), Plist::Boolean(true));
    unit.insert("UserName".to_string(), text(&account));
    unit.insert("StandardOutPath".to_string(), text(&log));
    unit.insert("StandardErrorPath".to_string(), text(&log));
    let mut bytes = Vec::new();
    Plist::Dictionary(unit)
        .to_writer_xml(&mut bytes)
        .map_err(|error| DeployError(format!("cannot render the object API unit: {error}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
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
        crate::config::QUEUE_OBJECT_NAMESPACE,
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
