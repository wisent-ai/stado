//! `stado host vault ...` and `stado host grant ...`.

pub(in crate::cli::host) mod consolidate;
pub(in crate::cli::host) mod grants;
pub(in crate::cli::host) mod item;
pub(in crate::cli::host) mod mint;
pub(in crate::cli::host) mod mirror;
pub(in crate::cli::host) mod rebind;
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
/// `stado credentials vault list [--host TARGET]` — which Skarbiec vaults the fleet holds.
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
        let resolved = crate::cli::canonical_host(name).await?;
        let answer = crate::deploy::fleet_vaults::collect_from(&resolved, &runner).await;
        let mut host = crate::deploy::fleet_vaults::attribute(name, answer);
        // What the host itself declares, read from the host: the operator's
        // laptop config is not the answer for a machine the operator is
        // asking about. `None` means the field was not in the answer at all,
        // which a release older than the key produces.
        // A configuration that cannot be read is said as such: read as no
        // configuration, the host was refused as declaring no vault authority.
        let shown = remote_config_output(&resolved, RemoteConfigAction::Show, &runner)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{name}: its Stado configuration could not be read, so its vault authority \
                     is not known: {error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        let resolved_config = serde_json::from_str::<Value>(&shown).map_err(|error| {
            CmdError::click(format!(
                "{name}: `stado config show` answered no readable JSON, so its vault authority \
                 is not known: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let resolved_key = |key: &str| {
            resolved_config
                .pointer(&format!("/resolved/{key}"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let declared = resolved_key("skarbiec_vault_file");
        // A host that declares no vault file but routes to the owner through
        // secrets.skarbiec.url is a client: it holds no vault by design, and
        // the vault files it still has are the copies to account for. Only a
        // host that declares neither has no authority at all.
        let owner_route = resolved_key("skarbiec_url");
        if declared.is_none() && owner_route.is_none() {
            return Err(CmdError::declaration(format!(
                "{name} declares no vault authority; add it to secrets.skarbiec.vault_file"
            )));
        }
        if let Some(object) = host.as_object_mut() {
            // A host whose probe reported no vaults holds none; the
            // authority verdict is still written so the row says so.
            let list: Vec<Value> = match object.get("vaults").and_then(Value::as_array) {
                Some(vaults) => vaults.clone(),
                None => Vec::new(),
            };
            object.insert(
                "authority".to_string(),
                match (&declared, &owner_route) {
                    (None, Some(route)) => {
                        crate::credential_store::owner::reads_owner(route, &list)
                    }
                    _ => crate::credential_store::owner::authority(declared.as_deref(), &list),
                },
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
        let name = match host.get("target").and_then(Value::as_str) {
            Some(name) => name,
            None => "a host row without its target",
        };
        if let Some(error) = host.get("error").and_then(Value::as_str) {
            println!("{name}: {error}");
            continue;
        }
        if let Some(absent) = host.get("absent").and_then(Value::as_str) {
            println!("{name}: {absent}");
            continue;
        }
        // A host row that reports no vault list holds none: the listing
        // reads `vaults` only where the probe wrote it.
        let list: &[Value] = match host.get("vaults").and_then(Value::as_array) {
            Some(vaults) => vaults.as_slice(),
            None => &[],
        };
        // The authority verdict names the path it chose; a verdict without
        // one (no authority) marks no line.
        let authority = host.get("authority");
        let chosen: Option<&str> = authority
            .and_then(|verdict| verdict.get("path"))
            .and_then(Value::as_str);
        println!("{name}: {} vault(s)", list.len());
        for vault in list {
            let path = match vault.get("path").and_then(Value::as_str) {
                Some(path) => path.to_string(),
                None => "(path not reported)".to_string(),
            };
            // The marked line is the one every owner write and authoritative
            // read on that host goes through. Reading a count without it told
            // an operator how much is held and nothing about which store
            // answers.
            let marker = if chosen == Some(path.as_str()) {
                "*"
            } else {
                " "
            };
            // A count the inventory did not report is said so, not printed
            // as an empty vault.
            let count = |key: &str| match vault.get(key).and_then(Value::as_u64) {
                Some(count) => count.to_string(),
                None => format!("(no {key} count)"),
            };
            println!(
                "{marker} {} items  {} recipients  {path}",
                count("items"),
                count("recipients"),
            );
        }
        if let Some(verdict) = authority {
            let state = match verdict.get("state").and_then(Value::as_str) {
                Some(state) => state.to_string(),
                None => "state not reported".to_string(),
            };
            match verdict.get("detail").and_then(Value::as_str) {
                Some(detail) => println!("  authority: {state} — {detail}"),
                None => println!("  authority: {state}"),
            }
        }
    }
    // A total the summary did not report is said so: zero unreachable hosts
    // read as an all-clear.
    let total = |key: &str| match summary.get(key).and_then(Value::as_u64) {
        Some(total) => total.to_string(),
        None => format!("(no {key} count)"),
    };
    println!(
        "{} host(s), {} unreachable, {} vault(s), {} item(s)",
        total("hosts"),
        total("unreachable"),
        total("vaults"),
        total("items"),
    );
    Ok(())
}

/// A vault item id or tag: the alphabet `release_component` allows, plus the
/// `:` these names are built out of (`provider:<provider>:<subscription>`,
/// `<product>:agent:<consumer>`) and the `@` and `+` of the account address
/// a tag such as `brama:account:<address>` carries. Brama writes that tag
/// through Skarbiec itself; a retag that refused it could not hand back the
/// tags an item already held, so `brama:login:` could not be corrected
/// without first dropping the account beside it.
///
/// Checked here because these words are interpolated into a script that
/// performs an owner write, and a name that arrived from an inventory is no
/// more trustworthy than one an operator typed.
pub(crate) fn vault_word(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'@' | b'+')
        })
    {
        return Err(CmdError::usage(format!(
            "{kind} must contain only letters, digits, '.', '_', '-', ':', '@' or '+'"
        )));
    }
    Ok(())
}
