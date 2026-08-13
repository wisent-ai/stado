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
    # A release older than the `vaults` subcommand is a fact about the
    # fleet's rollout, not a broken host, and must not surface as an
    # unparseable answer.
    if answer=$("$candidate" vaults 2>/dev/null); then
      printf '%s\n' "$answer"
      exit 0
    fi
    printf '{"host":"%s","vaults":[],"absent":"skarbiec here predates the vaults command"}\n' "$(hostname)"
    exit 0
  fi
done
printf '{"host":"%s","vaults":[],"absent":"no skarbiec binary on this host"}\n' "$(hostname)"
"#;

/// Ask one host. A host that cannot be reached is reported as unreachable
/// rather than dropped, because a fleet inventory missing a machine silently
/// is worse than one that says which machine it could not read.
pub async fn collect_from(target: &crate::targets::ComputeTarget, runner: &Runner) -> Value {
    match host_channel::run_script(target, REMOTE_VAULT_SCRIPT, runner).await {
        Ok(output) => match serde_json::from_str::<Value>(output.stdout.trim()) {
            Ok(document) => document,
            Err(error) => json!({
                "host": target.name.clone(),
                "vaults": [],
                "error": format!("unreadable answer: {error}"),
            }),
        },
        Err(DeployError(detail)) => json!({
            "host": target.name.clone(),
            "vaults": [],
            "error": detail,
        }),
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
    let mut vaults = usize::MIN;
    let mut items = u64::MIN;
    let mut unreachable = usize::MIN;
    let one = usize::from(u8::from(true));
    for host in hosts {
        if host.get("error").is_some() {
            unreachable = unreachable.saturating_add(one);
        }
        let Some(list) = host.get("vaults").and_then(Value::as_array) else {
            continue;
        };
        vaults = vaults.saturating_add(list.len());
        for vault in list {
            items = items.saturating_add(
                vault
                    .get("items")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
        }
    }
    json!({
        "hosts": hosts.len(),
        "unreachable": unreachable,
        "vaults": vaults,
        "items": items,
    })
}
