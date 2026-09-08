use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::deploy::service::*;

/// Atomically replace one assignment in an owner-controlled remote env file.
/// The value travels only inside the approved host-channel request body.
pub async fn set_env_key_on_host(
    target: &ComputeTarget,
    env_path: &str,
    key: &str,
    value: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() { printf 'env_set_failed\t%s\n' "$1"; exit 1; }
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) fail 'target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) fail 'target must be inside the target home' ;; esac
[ ! -L "$env_path" ] || fail 'target cannot be a symlink'
[ -f "$env_path" ] || fail 'environment file must already exist'
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved target leaves the target home'
key=$(printf '%s' '@KEY_B64@' | /usr/bin/base64 "$decode")
value=$(printf '%s' '@VALUE_B64@' | /usr/bin/base64 "$decode")
tmp="$parent/.stado-env-set.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
/usr/bin/awk -v key="$key" '$0 !~ "^" key "=" { print }' "$env_path" > "$tmp"
printf '%s=%s\n' "$key" "$value" >> "$tmp"
/bin/chmod 0600 "$tmp"
/bin/mv -f "$tmp" "$env_path"
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tenv-set\tenv_set\t%s\n' "$env_path"
"#;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()))
        .replace("@VALUE_B64@", &STANDARD.encode(value.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Atomically remove one assignment from an owner-controlled remote env file.
pub async fn unset_env_key_on_host(
    target: &ComputeTarget,
    env_path: &str,
    key: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() { printf 'env_unset_failed\t%s\n' "$1"; exit 1; }
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) fail 'target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) fail 'target must be inside the target home' ;; esac
[ ! -L "$env_path" ] || fail 'target cannot be a symlink'
[ -f "$env_path" ] || fail 'environment file must already exist'
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved target leaves the target home'
key=$(printf '%s' '@KEY_B64@' | /usr/bin/base64 "$decode")
tmp="$parent/.stado-env-unset.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
/usr/bin/awk -v key="$key" '$0 !~ "^" key "=" { print }' "$env_path" > "$tmp"
/bin/chmod 0600 "$tmp"
/bin/mv -f "$tmp" "$env_path"
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tenv-unset\tenv_unset\t%s\n' "$env_path"
"#;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Write one vault item field on the host using its own Skarbiec binary
/// and vault file. The value file must already exist on the host.
pub async fn set_item_field_on_host(
    target: &ComputeTarget,
    item: &str,
    field: &str,
    value_file: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GNUPGHOME="$HOME/.gnupg"
"$HOME/.stado/bin/skarbiec" set-json "$STADO_ITEM" --field "$STADO_FIELD" --from-file "$STADO_FROM"
echo 'STADO_ITEM_SET	ok'"#;
    let body = body
        .replace("{vault_file}", "")
        .replace("$STADO_ITEM", &shlex_quote(item))
        .replace("$STADO_FIELD", &shlex_quote(field))
        .replace("$STADO_FROM", &shlex_quote(value_file));
    let output = host_channel::run_script(target, &body, runner).await?;
    if !output.stdout.contains("STADO_ITEM_SET") {
        return Err(DeployError(format!(
            "{}: could not set {}.{}: {}",
            target.name,
            item,
            field,
            output.stderr.trim_end()
        )));
    }
    Ok(report_from(output))
}
