//! `stado host vault ...` and `stado host grant ...`.

pub(in crate::cli::host) mod grants;
pub(in crate::cli::host) mod item;
pub(in crate::cli::host) mod mirror;
pub(in crate::cli::host) mod token;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;
use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};

/// `stado host inventory TARGET [--json]` — the stado-managed binaries,
/// fixed Cargo-home metadata and bin membership, forward markers and loopback
/// listeners of TARGET, and the verdict on whether each marker still matches
/// a live listener.
///
/// The only thing it takes is the registry target name. There is no path,
/// file name, port or pattern to pass, because a command that took one
/// would be a command that could be pointed at `~/.ssh/id_ed25519`.
/// `stado credentials vaults [--host TARGET]` — which Skarbiec vaults the fleet holds.
///
/// Without a target this asks every registry host, because "how many vaults
/// does this fleet have" is the question a machine cannot answer about
/// itself: a vault is a file, and the desktop client that lists them is
/// honest that it only sees the machine it runs on.
///
/// Only an owner, three counts and a path cross the wire. Skarbiec's own
/// documentation calls item, consumer and scope names "the map" and holds
/// them above the encrypted values in confidentiality, so a fleet sweep
/// reports how much is held and never what.
pub async fn vaults(target: Option<String>, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let names: Vec<String> = match target {
        Some(name) => vec![name],
        None => {
            let registry = crate::cli::registry::read_registry().await?;
            registry
                .targets
                .iter()
                .map(|entry| entry.name.clone())
                .collect()
        }
    };
    let mut hosts: Vec<serde_json::Value> = Vec::new();
    for name in &names {
        let resolved = crate::deploy::host_channel::canonical_target(name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let answer = crate::deploy::fleet_vaults::collect_from(&resolved, &runner).await;
        let mut host = crate::deploy::fleet_vaults::attribute(name, answer);
        // What the host itself declares, read from the host: the operator's
        // laptop config is not the answer for a machine the operator is
        // asking about. `None` means the field was not in the answer at all,
        // which a release older than the key produces.
        let declared = remote_config_output(&resolved, RemoteConfigAction::Show, &runner)
            .await
            .ok()
            .and_then(|stdout| serde_json::from_str::<Value>(&stdout).ok())
            .and_then(|document| {
                document
                    .pointer("/resolved/skarbiec_vault_file")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
        if declared.is_none() {
            return Err(CmdError::click(format!(
                "{name} declares no vault authority; add it to secrets.skarbiec.vault_file"
            )));
        }
        if let Some(object) = host.as_object_mut() {
            let list = object
                .get("vaults")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            object.insert(
                "authority".to_string(),
                crate::credential_store::owner::authority(declared.as_deref(), &list),
            );
        }
        hosts.push(host);
    }
    let summary = crate::deploy::fleet_vaults::summarize(&hosts);
    if json {
        print_json(&json!({"summary": summary, "hosts": hosts}));
        return Ok(());
    }
    for host in &hosts {
        let name = host.get("target").and_then(Value::as_str).unwrap_or("?");
        if let Some(error) = host.get("error").and_then(Value::as_str) {
            println!("{name}: {error}");
            continue;
        }
        if let Some(absent) = host.get("absent").and_then(Value::as_str) {
            println!("{name}: {absent}");
            continue;
        }
        let list = host
            .get("vaults")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let authority = host.get("authority");
        let chosen = authority
            .and_then(|verdict| verdict.get("path"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        println!("{name}: {} vault(s)", list.len());
        for vault in list {
            let path = vault.get("path").and_then(Value::as_str).unwrap_or("");
            // The marked line is the one every owner write and authoritative
            // read on that host goes through. Reading a count without it told
            // an operator how much is held and nothing about which store
            // answers.
            let marker = if !chosen.is_empty() && path == chosen {
                "*"
            } else {
                " "
            };
            println!(
                "{marker} {:>5} items  {} recipients  {}",
                vault
                    .get("items")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                vault
                    .get("recipients")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                path
            );
        }
        if let Some(verdict) = authority {
            println!(
                "  authority: {} — {}",
                verdict
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                verdict.get("detail").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    println!(
        "{} host(s), {} unreachable, {} vault(s), {} item(s)",
        summary
            .get("hosts")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("unreachable")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("vaults")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("items")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    );
    Ok(())
}

/// A vault item id or tag: the alphabet `release_component` allows, plus the
/// `:` that every one of these names is built out of
/// (`provider:kimi:brama-sub-…`, `brama:agent:wisent-app`).
///
/// Checked here because these words are interpolated into a script that
/// performs an owner write, and a name that arrived from an inventory is no
/// more trustworthy than one an operator typed.
pub(in crate::cli::host) fn vault_word(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(CmdError::usage(format!(
            "{kind} must contain only letters, digits, '.', '_', '-' or ':'"
        )));
    }
    Ok(())
}
