//! `stado fleet vaults` — which Skarbiec vaults each registry host holds.
//!
//! A vault is a file, so "how many vaults does this fleet have" is a question
//! no single machine can answer. The desktop client answers it for the
//! machine it runs on and says so; this answers it for the fleet, through the
//! same registry-authorized channel as every other host command.
//!
//! Two properties are deliberate.
//!
//! **No secret material and no item names cross the wire.** The remote side
//! runs `skarbiec vaults`, which reads each vault's plaintext envelope for an
//! owner and three counts and never decrypts a value. Skarbiec's own product
//! documentation calls the names of items, consumers and scopes "the map" and
//! holds them to a higher confidentiality bar than the encrypted values, so a
//! fleet-wide collection is the last place to widen that exposure.
//!
//! **Nothing is installed to ask.** The remote script is a compile-time
//! constant that locates an already-installed `skarbiec` and runs one
//! read-only subcommand. A host without Skarbiec answers that it has none,
//! which is a fact about the fleet rather than an error.

use serde_json::{json, Value};

use super::{host_channel, DeployError, Runner};

/// Locate this host's Skarbiec and ask it for its vaults.
///
/// The binary's path differs per machine — it is under the owner's home on
/// every host in this fleet — so a fixed absolute path in the host-exec
/// allowlist could not express it. The candidates below are the paths the
/// product's own installer and release channel use.
const REMOTE_VAULT_SCRIPT: &str = r#"set -eu
for candidate in \
  "$HOME/.stado/bin/skarbiec" \
  "$HOME/.local/bin/skarbiec" \
  /opt/homebrew/bin/skarbiec \
  /usr/local/bin/skarbiec
do
  if [ -x "$candidate" ]; then
    # What Skarbiec says on stderr is kept: a release older than the
    # `vaults` subcommand and a vault it cannot read are different
    # answers, and both are told in its own words.
    /bin/mkdir -p "$HOME/.stado"
    errors=$(/usr/bin/mktemp "$HOME/.stado/fleet-vaults-errors.XXXXXX")
    trap '/bin/rm -f "$errors"' EXIT
    if answer=$("$candidate" vaults 2>"$errors"); then
      printf '%s\n' "$answer"
      exit 0
    fi
    printf 'STADO_VAULTS_REFUSED\t%s\t%s\n' "$candidate" "$(/usr/bin/tr '\n\t' '  ' < "$errors")"
    exit 0
  fi
done
printf '{"host":"%s","vaults":[],"absent":"no skarbiec binary on this host"}\n' "$(hostname)"
"#;

/// Ask one host. A host that cannot be reached is reported as unreachable
/// rather than dropped, because a fleet inventory missing a machine silently
/// is worse than one that says which machine it could not read.
pub async fn collect_from(target: &crate::targets::ComputeTarget, runner: &Runner) -> Value {
    let failed = |error: String| {
        json!({
            "host": target.name.clone(),
            "vaults": [],
            "error": error,
        })
    };
    let output = match host_channel::run_script(target, REMOTE_VAULT_SCRIPT, runner).await {
        Ok(output) => output,
        Err(DeployError {
            message: detail, ..
        }) => return failed(detail),
    };
    let answer = output.stdout.trim();
    // The trimmed answer loses the trailing tab of a refusal that printed no
    // words, so the words are whatever fields follow the binary.
    if let ["STADO_VAULTS_REFUSED", binary, words @ ..] =
        host_channel::marker_fields(answer).as_slice()
    {
        let said = words.join(" ");
        return failed(if said.trim().is_empty() {
            format!("{binary} vaults failed and printed nothing on stderr")
        } else {
            format!("{binary} vaults failed: {}", said.trim())
        });
    }
    match serde_json::from_str::<Value>(answer) {
        Ok(document) => document,
        Err(error) => failed(format!(
            "unreadable answer ({error}): {answer}; {}",
            host_channel::whole_failure(&output)
        )),
    }
}

/// Fold one host's answer into the fleet report, labelling it with the
/// registry name so two hosts that report the same `hostname` stay distinct.
pub fn attribute(target_name: &str, mut answer: Value) -> Value {
    if let Some(object) = answer.as_object_mut() {
        object.insert("target".to_string(), json!(target_name));
    }
    answer
}

/// Totals an operator reads before the detail.
pub fn summarize(hosts: &[Value]) -> Value {
    let unreachable: Vec<&Value> = hosts
        .iter()
        .filter(|host| host.get("error").is_some())
        .collect();
    let vaults: Vec<&Value> = hosts
        .iter()
        .filter_map(|host| host.get("vaults").and_then(Value::as_array))
        .flatten()
        .collect();
    let items: u64 = vaults
        .iter()
        .filter_map(|vault| vault.get("items").and_then(Value::as_u64))
        .sum();
    json!({
        "hosts": hosts.len(),
        "unreachable": unreachable.len(),
        "vaults": vaults.len(),
        "items": items,
    })
}
