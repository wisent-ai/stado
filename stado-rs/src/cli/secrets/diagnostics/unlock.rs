//! Testing surviving unlock phrases against a local or remote vault, and
//! reporting which source name worked — never the phrase.

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::secrets::store::resolve::skarbiec_binary;

/// Test every unlock phrase the transcripts still hold against the vault.
///
/// The oracle is Skarbiec's own `key-doctor`, run once per candidate with the
/// phrase in its environment: if the canary item opens, that phrase is the one.
/// Reusing the existing verdict means no second decryption path and no crypto
/// written here.
///
/// Reports the SOURCE NAME of the phrase that worked, never the phrase. A
/// passphrase that leaked into a transcript should not also be printed to a
/// terminal by the tool that found it.
pub(crate) async fn try_unlock(host: Option<&str>, keychain_only: bool) -> Result<(), CmdError> {
    let candidates = if keychain_only {
        Vec::new()
    } else {
        crate::transcripts::unlock_candidates()
    };
    if candidates.is_empty() && !keychain_only {
        return Err(CmdError::click(
            "no unlock phrase of any kind survives in transcript runtime output",
        ));
    }
    match host {
        Some(host) => try_unlock_remote(host, &candidates, keychain_only).await,
        None => try_unlock_local(&candidates),
    }
}

fn try_unlock_local(candidates: &[(String, String)]) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    println!(
        "testing {} distinct phrase(s) from transcript history",
        candidates.len()
    );
    for (name, phrase) in candidates {
        let output = std::process::Command::new(&binary)
            .arg("key-doctor")
            .env("SKARBIEC_UNLOCK", phrase)
            .output()?;
        let report: Value = match serde_json::from_slice(&output.stdout) {
            Ok(report) => report,
            Err(_) => continue,
        };
        if let Some("readable") = report.get("status").and_then(Value::as_str) {
            println!("the vault OPENS with the phrase recorded under {name}");
            println!("set it as SKARBIEC_UNLOCK, then rotate-owner onto a key you control");
            return Ok(());
        }
    }
    Err(CmdError::click(format!(
        "none of the {} surviving phrase(s) opens the vault: the protected key's passphrase is not in any transcript",
        candidates.len()
    )))
}

async fn try_unlock_remote(
    host: &str,
    candidates: &[(String, String)],
    keychain_only: bool,
) -> Result<(), CmdError> {
    use base64::Engine as _;

    let target = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut encoded = String::new();
    for (name, phrase) in candidates {
        encoded.push_str(&base64::engine::general_purpose::STANDARD.encode(name.as_bytes()));
        encoded.push(' ');
        encoded.push_str(&base64::engine::general_purpose::STANDARD.encode(phrase.as_bytes()));
        encoded.push('\n');
    }
    let keychain_source =
        base64::engine::general_purpose::STANDARD.encode(b"macOS Keychain service skarbiec-vault");
    let failure = if keychain_only {
        "the host keychain entry does not open the remote vault"
    } else {
        "neither the host keychain nor any surviving transcript phrase opens the remote vault"
    };
    let script = format!(
        r#"set -euo pipefail
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
binary="$HOME/.stado/bin/skarbiec"
vault="$HOME/.stado/skarbiec.vault.json"
unlock="$HOME/.stado/skarbiec-unlock"
try_phrase() {{
  source_b64="$1"
  phrase="$2"
  set +e
  report="$(GNUPGHOME="$HOME/.gnupg" SKARBIEC_VAULT_FILE="$vault" SKARBIEC_UNLOCK="$phrase" "$binary" key-doctor 2>/dev/null)"
  status=$?
  set -e
  if [ "$status" -eq 0 ] && printf '%s' "$report" | /usr/bin/grep -q '"status"[[:space:]]*:[[:space:]]*"readable"'; then
    umask 077
    printf '%s' "$phrase" > "$unlock.new"
    /bin/chmod 600 "$unlock.new"
    /bin/mv -f "$unlock.new" "$unlock"
    printf 'STADO_UNLOCK\t%s\n' "$source_b64"
    exit 0
  fi
}}
while IFS= read -r candidate; do
  [ "$candidate" != "$vault" ] || continue
  set +e
  report="$(GNUPGHOME="$HOME/.gnupg" SKARBIEC_VAULT_FILE="$candidate" "$binary" key-doctor 2>/dev/null)"
  status=$?
  set -e
  if [ "$status" -eq 0 ] && printf '%s' "$report" | /usr/bin/grep -q '"status"[[:space:]]*:[[:space:]]*"readable"'; then
    stamp="$(/bin/date -u +%Y%m%dT%H%M%SZ)"
    /bin/cp -p "$vault" "$vault.unreadable-$stamp"
    /bin/cp -p "$candidate" "$vault.new"
    /bin/chmod 600 "$vault.new"
    /bin/mv -f "$vault.new" "$vault"
    printf 'STADO_BACKUP\t%s\n' "$candidate"
    exit 0
  fi
done < <(/usr/bin/find "$HOME/.stado" -maxdepth 4 -type f \( -name '*skarbiec*vault*.json*' -o -name '*skarbiec*.bak' \) -print)
if [ "$(/usr/bin/uname -s)" = Darwin ]; then
  keychain_phrase="$(/bin/launchctl asuser "$(/usr/bin/id -u)" /usr/bin/security find-generic-password -s skarbiec-vault -w 2>/dev/null || true)"
  if [ -z "$keychain_phrase" ]; then
    keychain_phrase="$(/usr/bin/security find-generic-password -s skarbiec-vault -w 2>/dev/null || true)"
  fi
  if [ -n "$keychain_phrase" ]; then
    try_phrase "{keychain_source}" "$keychain_phrase"
  fi
fi
while IFS=' ' read -r source_b64 phrase_b64; do
  [ -n "$source_b64" ] || continue
  phrase="$(printf '%s' "$phrase_b64" | /usr/bin/base64 "$decode")"
  try_phrase "$source_b64" "$phrase"
done <<'STADO_UNLOCK_CANDIDATES'
{encoded}STADO_UNLOCK_CANDIDATES
printf '%s\n' '{failure}' >&2
exit 2
"#
    );
    if keychain_only {
        println!("testing the host keychain against the vault on {host}");
    } else {
        println!(
            "testing the host keychain and {} distinct transcript phrase(s) against the vault on {host}",
            candidates.len()
        );
    }
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &target,
        &script,
        if keychain_only {
            std::time::Duration::from_secs(30)
        } else {
            std::time::Duration::from_secs(900)
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(
            crate::deploy::host_channel::last_error_line(
                &output,
                "remote vault unlock recovery failed",
            ),
        ));
    }
    if let Some(path) = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_BACKUP\t"))
    {
        println!("restored the readable vault backup {path} on {host}");
        println!("preserved the unreadable vault beside it with a UTC suffix");
        return Ok(());
    }
    let encoded_name = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNLOCK\t"))
        .ok_or_else(|| CmdError::click("remote unlock recovery returned no source marker"))?;
    let name = base64::engine::general_purpose::STANDARD
        .decode(encoded_name)
        .map_err(|error| CmdError::click(format!("remote unlock source is invalid: {error}")))?;
    println!(
        "the vault on {host} OPENS with the phrase recorded under {}",
        String::from_utf8_lossy(&name)
    );
    println!("stored it in the host's owner-only persistent unlock file");
    Ok(())
}
