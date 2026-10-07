use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::deploy::service::*;

/// A systemd definition or one drop-in belonging to this declared unit.
pub fn is_systemd_env_file(service: &ManagedService, path: &str) -> bool {
    service.kind == KIND_SYSTEMD
        && !service.path.is_empty()
        && (path == service.path
            || path
                .strip_prefix(&service.path)
                .and_then(|suffix| suffix.strip_prefix(".d/"))
                .is_some_and(|name| {
                    name.ends_with(".conf")
                        && name.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                        })
                }))
}

/// Update a declared systemd environment assignment without cycling the unit.
/// `None` removes the assignment and an otherwise empty drop-in.
pub async fn set_unit_env_key_on_host(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    key: &str,
    value: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !is_systemd_env_file(service, env_path) {
        return Err(
            DeployError("environment file does not belong to this systemd unit".into())
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
    }
    let body = r###"stado_unit_env_writer() {
  if [ "$scope" = system ]; then
    stado_root "$@"
  elif [ "$service_uid" = "$uid" ]; then
    "$@"
  else
    "$sudo_bin" -n -u "$service_user" "$@"
  fi
}
stado="$HOME/.stado/bin/stado"
[ -x "$stado" ] || stado="$(command -v stado)"
# The service account may not reach this account's home. It gets its own
# readable copy of the same Stado for this one edit, removed right after.
staged=''
if ! stado_unit_env_writer /bin/sh -c '[ -x "$1" ]' sh "$stado" 2>/dev/null; then
  staged=$(/bin/mktemp -d /tmp/stado-unit-env.XXXXXX) || { say '@ACTION@_failed' 'cannot stage Stado for the unit account'; exit 1; }
  /bin/chmod 755 "$staged" && /bin/cp "$stado" "$staged/stado" && /bin/chmod 755 "$staged/stado" \
    || { /bin/rm -rf "$staged"; say '@ACTION@_failed' 'cannot stage Stado for the unit account'; exit 1; }
  stado="$staged/stado"
fi
written=0
# The value is piped by the shell's own printf builtin, so it is never an
# argument of any process on the host.
changed=$(printf '%s' '@VALUE_B64@' | stado_unit_env_writer "$stado" service unit-env-local --path-b64 '@ENV_PATH_B64@' --key-b64 '@KEY_B64@' @VALUE_ARG@ --uid "$service_uid" 2>&1) || written=$?
[ -z "$staged" ] || /bin/rm -rf "$staged"
if [ "$written" != 0 ]; then
  say '@ACTION@_failed' "$(printf '%s' "$changed" | tr '\t\r\n' '   ')"
  exit 1
fi
if [ "$changed" = changed ] || [ "$(stado_systemctl show -p NeedDaemonReload --value "$unit")" = yes ]; then
  if ! stado_systemctl daemon-reload; then
    say '@ACTION@_failed' 'unit environment changed but systemd could not reload it'
    exit 1
  fi
fi
say '@ACTION@' "$changed; systemd definition refreshed without restarting the unit"
"###;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()))
        .replace(
            "@VALUE_ARG@",
            if value.is_some() { "--value-stdin" } else { "" },
        )
        .replace(
            "@VALUE_B64@",
            &value
                .map(|value| STANDARD.encode(value.as_bytes()))
                .unwrap_or_default(),
        )
        .replace(
            "@ACTION@",
            if value.is_some() {
                "env_set"
            } else {
                "env_unset"
            },
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}
