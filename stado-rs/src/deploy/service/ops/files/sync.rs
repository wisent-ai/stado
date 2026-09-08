use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::deploy::service::*;

/// Atomically replace one owner-only file on a managed host.
///
/// The content rides inside the approved channel's request body as base64,
/// never argv or output. The destination stays under the target account's
/// real home and a symlink is refused rather than followed.
pub async fn sync_service_file(
    target: &ComputeTarget,
    target_path: &str,
    content: &[u8],
    mode: u32,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !matches!(mode, 0o600 | 0o700) {
        return Err(DeployError(format!(
            "service file mode must be 0600 or 0700, got {mode:04o}"
        )));
    }
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\tfile-sync\tfile_sync_failed\t%s\n' "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
target_path=$(printf '%s' '@TARGET_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$target_path" in
  \$HOME/*) target_path="$HOME/${target_path#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'target path must be under the target home' ;;
esac
if [ -e "$target_path" ] || [ -L "$target_path" ]; then
  [ -f "$target_path" ] && [ ! -L "$target_path" ] || fail 'target path is not a regular file'
fi
parent=$(/usr/bin/dirname "$target_path") || fail 'target parent unavailable'
/bin/mkdir -p "$parent" || fail 'cannot create target parent'
if ! /usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=os.path.realpath(sys.argv[2]); raise SystemExit(0 if os.path.commonpath((home,parent)) == home else 1)' "$HOME" "$parent"; then
  fail 'target parent escapes the target home'
fi
tmp="$target_path.stado-file-sync.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
umask u=rw,go=
printf '%s' '@CONTENT_B64@' | /usr/bin/base64 "$decode" > "$tmp" || fail 'cannot stage file'
/bin/chmod @MODE@ "$tmp" || fail 'cannot protect file'
/bin/mv -f "$tmp" "$target_path" || fail 'cannot install file'
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tfile-sync\tfile_synced\t%s\n' "$target_path"
"#;
    let body = body
        .replace(
            "@TARGET_PATH_B64@",
            &STANDARD.encode(target_path.as_bytes()),
        )
        .replace("@CONTENT_B64@", &STANDARD.encode(content))
        .replace("@MODE@", &format!("{mode:04o}"));
    let output =
        host_channel::run_script_with_timeout(target, &body, sync_timeout(content.len()), runner)
            .await?;
    Ok(report_from(output))
}

/// Create, or atomically replace, one owner-only raw bearer file on a managed
/// host.
///
/// Binding a host's unit to the fleet object store needs
/// `WC_STADO_STORAGE_TOKEN_FILE` to name a file whose entire content is the
/// bearer: `queue/stado_object.rs::StadoObjectBackend::new` resolves a token
/// file and nothing else. Nothing here could create that file.
/// [`sync_service_secret`] writes an `env` assignment, and
/// [`remint_consumer_grant_on_host`] reconciles a grant against a token file
/// that is already on the host; between them the one remaining way to bind a
/// host to the fleet store was to hand-copy a secret onto it, which is exactly
/// what the fleet-wide "everything through Stado" rule exists to prevent. A
/// host that never got that file kept its `JobStorage` on a device-local store
/// instead, and a fleet claim written to a device store does not fail -- it
/// succeeds, and every other host reports the object absent.
///
/// The bearer rides inside the approved channel's request body as base64,
/// exactly the way [`sync_service_file`] carries file content: never an
/// argument vector, never stdout, never the clear text of the remote program.
/// The destination must resolve inside the target account's home and must not
/// be a symlink, its resolved parent must still be inside that home, and the
/// file is installed by renaming a mode-600 temporary file staged in the same
/// directory, so a reader never sees a half-written bearer. Unlike
/// [`set_env_key_on_host`] an absent destination is created -- that is the
/// whole point of this command -- but an absent parent directory is refused
/// rather than created: a bearer written into a directory this command invented
/// is a bearer nothing reads, and the typo that put it there would be reported
/// as a successful sync.
pub async fn write_token_file_on_host(
    target: &ComputeTarget,
    token_path: &str,
    secret: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(token_path, "token file")?;
    validate_secret_value(secret)?;
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\ttoken-file-sync\ttoken_file_sync_failed\t%s\n' "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
token_path=$(printf '%s' '@TOKEN_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$token_path" in
  '$HOME'/*) token_path="$home/${token_path#\$HOME/}" ;;
  "$home"/*) ;;
  *) fail 'token file must be inside the target home' ;;
esac
[ ! -L "$token_path" ] || fail 'token file cannot be a symlink'
if [ -e "$token_path" ]; then
  [ -f "$token_path" ] || fail 'token file is not a regular file'
fi
parent=$(/usr/bin/dirname "$token_path")
[ -d "$parent" ] || fail 'token file parent directory must already exist'
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved token file leaves the target home'
tmp="$parent/.stado-token-file-sync.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
umask u=rw,go=
printf '%s' '@TOKEN_B64@' | /usr/bin/base64 "$decode" > "$tmp" || fail 'cannot stage token file'
[ -s "$tmp" ] || fail 'staged token file is empty'
/bin/chmod 0600 "$tmp" || fail 'cannot protect token file'
/bin/mv -f "$tmp" "$token_path" || fail 'cannot install token file'
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\ttoken-file-sync\ttoken_file_synced\t%s\n' "$token_path"
"#;
    let body = body
        .replace("@TOKEN_PATH_B64@", &STANDARD.encode(token_path.as_bytes()))
        .replace("@TOKEN_B64@", &STANDARD.encode(secret.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Replace `consumer`'s complete grant against the target's authoritative
/// vault while preserving the bearer already held in `token_path`.
///
/// Both files stay on the managed host. Skarbiec reads the raw bearer itself
/// and records only its hash; the value never enters Stado output, argv, or
/// the control-plane process.
#[allow(clippy::too_many_arguments)]
pub async fn remint_consumer_grant_on_host(
    target: &ComputeTarget,
    consumer: &str,
    capabilities: &str,
    token_path: &str,
    vault_file: &str,
    ttl_seconds: u64,
    audience: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\t%s\tgrant_sync_failed\t%s\n' "$consumer" "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
vault=$(printf '%s' '@VAULT_B64@' | /usr/bin/base64 "$decode") || exit 1
consumer=$(printf '%s' '@CONSUMER_B64@' | /usr/bin/base64 "$decode") || exit 1
caps=$(printf '%s' '@CAPS_B64@' | /usr/bin/base64 "$decode") || exit 1
token_path=$(printf '%s' '@TOKEN_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
audience=$(printf '%s' '@AUDIENCE_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$vault" in
  \$HOME/*) vault="$HOME/${vault#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'vault path must be under the target home' ;;
esac
case "$token_path" in
  \$HOME/*) token_path="$HOME/${token_path#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'token path must be under the target home' ;;
esac
[ -f "$vault" ] && [ ! -L "$vault" ] || fail 'authoritative vault is not a regular file'
[ -f "$token_path" ] && [ ! -L "$token_path" ] || fail 'token file is not a regular file'
/bin/chmod 600 "$token_path" || fail 'cannot protect token file'
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GNUPGHOME="$HOME/.gnupg"
export SKARBIEC_VAULT_FILE="$vault"
if ! report=$("$HOME/.stado/bin/skarbiec" token-mint "$consumer" \
    --capabilities "$caps" \
    --token-file "$token_path" \
    --replace-capabilities \
    --ttl-seconds '@TTL_SECONDS@' \
    --audience "$audience" 2>&1); then
  fail "$report"
fi
printf 'STADO_SERVICE\t%s\tgrant_synced\t%s\n' "$consumer" "$token_path"
"#;
    let body = body
        .replace("@VAULT_B64@", &STANDARD.encode(vault_file.as_bytes()))
        .replace("@CONSUMER_B64@", &STANDARD.encode(consumer.as_bytes()))
        .replace("@CAPS_B64@", &STANDARD.encode(capabilities.as_bytes()))
        .replace("@TOKEN_PATH_B64@", &STANDARD.encode(token_path.as_bytes()))
        .replace("@AUDIENCE_B64@", &STANDARD.encode(audience.as_bytes()))
        .replace("@TTL_SECONDS@", &ttl_seconds.to_string());
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}
