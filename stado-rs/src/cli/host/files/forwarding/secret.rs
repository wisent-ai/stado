use base64::{engine::general_purpose::STANDARD, Engine as _};

use sha2::{Digest, Sha256};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::release_component;

pub(super) async fn transfer_secret(
    target: &str,
    name: &str,
    bytes: &[u8],
    home: Option<&str>,
) -> Result<(String, usize), CmdError> {
    release_component("secret file name", name)?;
    if bytes.is_empty() || bytes.len() > usize::from(u16::MAX) {
        return Err(CmdError::click(
            "host secret must contain between one and 65535 bytes",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(bytes);
    let expected_sha256 = hex::encode(digest.finalize());
    let payload = STANDARD.encode(bytes);
    let remote_name = crate::deploy::shlex_quote(name);
    let remote_expected = crate::deploy::shlex_quote(&expected_sha256);
    let remote_home = match home {
        Some(home) => {
            let valid = home.starts_with('/')
                && !home.chars().any(char::is_control)
                && !home
                    .split('/')
                    .any(|component| matches!(component, "." | ".."));
            if !valid {
                return Err(CmdError::usage(
                    "target home must be an absolute path without '.' or '..' components",
                ));
            }
            crate::deploy::shlex_quote(home)
        }
        None => "\"$HOME\"".to_string(),
    };
    let script = format!(
        r#"set -euo pipefail
name={remote_name}
expected={remote_expected}
home={remote_home}
case "$name" in
  ""|*[!A-Za-z0-9._-]*) printf '%s\n' 'invalid secret file name' >&2; exit 1 ;;
esac
if [ ! -d "$home" ]; then
  printf '%s\n' 'target home directory does not exist' >&2
  false
fi
os=$(/usr/bin/uname -s)
if [ "$os" = "Darwin" ]; then
  decode=-D
  owner=$(/usr/bin/stat -f %Su "$home")
  group=$(/usr/bin/stat -f %Sg "$home")
else
  decode=--decode
  owner=$(/usr/bin/stat -c %U "$home")
  group=$(/usr/bin/stat -c %G "$home")
fi
if [ -x /usr/bin/chown ]; then chown_bin=/usr/bin/chown; else chown_bin=/usr/sbin/chown; fi
current=$(/usr/bin/id -un)
if [ "$owner" != "$current" ] && [ "$(/usr/bin/id -u)" -ne 0 ]; then
  printf '%s\n' 'SSH account cannot write the selected target home' >&2
  false
fi
dir="$home/.stado"
tmp="$dir/.${{name}}.stado-secret.$$"
trap 'rm -f "$tmp"' EXIT
/bin/mkdir -p "$dir"
/bin/chmod 700 "$dir"
if [ "$owner" != "$current" ]; then "$chown_bin" "$owner:$group" "$dir"; fi
printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
/bin/chmod 600 "$tmp"
if [ "$owner" != "$current" ]; then "$chown_bin" "$owner:$group" "$tmp"; fi
/bin/mv "$tmp" "$dir/$name"
line=$(/usr/bin/openssl dgst -sha256 -r "$dir/$name")
actual="${{line%% *}}"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' 'secret transfer checksum mismatch' > /dev/stderr
  false
fi
trap - EXIT
printf '%s\n' "$dir/$name"
"#
    );
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: secret installation failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote secret write failed")
        )));
    }
    Ok((
        format!("{}/.stado/{name}", home.unwrap_or("$HOME")),
        bytes.len(),
    ))
}
