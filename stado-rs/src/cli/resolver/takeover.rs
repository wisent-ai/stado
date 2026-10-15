//! `stado placement move --authority --to-host HOST`, run on HOST: take the
//! registry authority role while the declared authority does not answer.
//!
//! The move every other placement makes commits through the authority, so a
//! silent authority could not be replaced. Here the host taking the role
//! promotes its own last-known-good registry: the service directory names it
//! as authority, with this program as the command resolvers run there, under
//! the next generation. That document becomes this host's copy (its resolver
//! adopts it and reads its own store from then on), is written into the
//! store this host reads, and is handed to every other registered host's
//! copy with `stado registry push - --last-known-good`, so each resolver
//! follows the new authority. The generation is the fence: the old
//! authority's later answers carry a lower one and are refused as a
//! rollback. Every step that did not land is named in the receipt and the
//! command fails; nothing is reported as done that was not.

use serde_json::{json, Value};

use crate::cli::resolver::authority::execute::execute_with_input;
use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::directory::document::{authority_snapshot, kept_last_good_document};
use crate::cli::resolver::directory::source::current_target;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::service_resolution;
use crate::targets::{self, RegistryStore};

/// The store version the promoted copy records: no store issued it yet.
const PROMOTED_VERSION: &str = "authority-takeover";

/// The state `registry push --last-known-good --json` answers with.
const HANDED_OVER: &str = "handed-over";

pub(crate) async fn take_authority(to_host: &str, json_output: bool) -> Result<(), CmdError> {
    let refused = |sentence: String| CmdError::click(sentence).stating(FailureCode::Refused);
    let config = |sentence: String| CmdError::click(sentence).stating(FailureCode::Config);
    let copy = kept_last_good_document().map_err(config)?.ok_or_else(|| {
        refused(
            "this host keeps no last-known-good registry to promote; the authority role is \
             taken on a host whose resolver has read the registry"
                .to_string(),
        )
    })?;
    let local = current_target(&copy).map_err(config)?;
    if local != to_host {
        return Err(refused(format!(
            "the authority role is taken on the host that takes it: run this on {to_host}, not {local}"
        )));
    }
    let directory = service_resolution::directory(&copy)
        .map_err(config)?
        .ok_or_else(|| {
            config("this host's registry copy declares no service_directory".to_string())
        })?;
    let old = directory.authority.target.clone();
    if old == to_host {
        return Err(refused(format!(
            "{to_host} is already the registry authority"
        )));
    }
    let unanswered = match authority_snapshot().await {
        Ok(Some(_)) => {
            return Err(refused(format!(
                "the registry authority {old} answers; move the role through it with \
                 `stado placement move` on {old}"
            )))
        }
        Ok(None) => {
            return Err(config(
                "this host's copy names no authority to take the role from".to_string(),
            ))
        }
        Err(error) => error.to_string(),
    };
    let program = std::env::current_exe().map_err(|error| {
        config(format!(
            "this program's own path, which resolvers would run here, cannot be read: {error}"
        ))
    })?;
    let command = program.to_str().ok_or_else(|| {
        config(format!(
            "this program's path {} is not UTF-8",
            program.display()
        ))
    })?;
    let mut promoted = copy.clone();
    let authority = promoted
        .pointer_mut("/service_directory/authority")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| config("service_directory.authority is not an object".to_string()))?;
    authority.insert("target".to_string(), json!(to_host));
    authority.insert("command".to_string(), json!(command));
    let generation = service_resolution::advance_generation(&mut promoted).map_err(config)?;
    let carried = targets::validate_registry_for_write(&promoted, Some(&copy))
        .map_err(|error| refused(format!("the promoted registry does not validate: {error}")))?;
    if let Some(detail) = carried {
        eprintln!(
            "stado placement move --authority: the promoted registry carries a section that was \
             already invalid in this host's copy, unchanged: {detail}"
        );
    }
    let content = serde_json::to_string_pretty(&promoted)?;
    targets::store_last_good(&content, PROMOTED_VERSION).map_err(|refusal| {
        config(format!(
            "this host's last-known-good copy refused the promoted registry: {refusal}"
        ))
    })?;
    let store = stored(&content).await;
    let registry =
        targets::load_registry_from_str(&content).map_err(|error| config(error.to_string()))?;
    let mut hosts = Vec::new();
    let mut not_handed = Vec::new();
    for target in &registry.targets {
        if target.name == to_host || target.name == old {
            continue;
        }
        match handed(target, &content).await {
            Ok(receipt) => hosts.push(json!({"host": target.name, "receipt": receipt})),
            Err(error) => {
                not_handed.push(target.name.clone());
                hosts.push(json!({"host": target.name, "error": error}));
            }
        }
    }
    let complete = store.is_ok() && not_handed.is_empty();
    let receipt = json!({
        "state": if complete { "taken" } else { "incomplete" },
        "authority": to_host,
        "fenced": old,
        "directory_generation": generation,
        "authority_unanswered": unanswered,
        "store": match &store {
            Ok(version) => json!({"version": version}),
            Err(error) => json!({"error": error}),
        },
        "hosts": hosts,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "{to_host} took the registry authority from {old} at directory generation {generation}"
        );
        println!("  store: {}", receipt["store"]);
        for row in &hosts {
            println!("  {row}");
        }
    }
    if complete {
        return Ok(());
    }
    let store_said = match store {
        Ok(version) => format!("the store was written ({version})"),
        Err(error) => format!("the store was not written ({error})"),
    };
    Err(CmdError::click(format!(
        "the authority role moved to {to_host}, but not everywhere: {store_said}; hosts not \
         handed the registry: [{}]",
        not_handed.join(", ")
    ))
    .stating(FailureCode::InfraDown))
}

/// Write the promoted registry into the store this host reads, which its
/// copy now names it the authority of: created when absent, else swapped
/// against the version it holds.
async fn stored(content: &str) -> Result<String, String> {
    let store = RegistryStore::open()
        .await
        .map_err(|error| error.to_string())?;
    match store
        .read_versioned()
        .await
        .map_err(|error| error.to_string())?
    {
        Some(current) => store
            .compare_and_swap(&current.version, content)
            .await
            .map_err(|error| error.to_string()),
        None => match store.create_if_absent(content).await {
            Ok(true) => Ok("created".to_string()),
            Ok(false) => {
                Err("a registry appeared in the store while it was being created".to_string())
            }
            Err(error) => Err(error.to_string()),
        },
    }
}

/// Hand `content` to one host's last-known-good copy over its SSH paths: the
/// host's receipt, or why no path took it.
async fn handed(target: &targets::ComputeTarget, content: &str) -> Result<Value, String> {
    let remote = format!(
        "{} registry push - --last-known-good --json",
        crate::deploy::host_exec::STADO_CLI
    );
    let mut failures = Vec::new();
    for path in target_ssh_paths(target) {
        let output = match execute_with_input(&path.destination, &remote, content.as_bytes()).await
        {
            Ok(output) => output,
            Err(error) => {
                failures.push(format!("{}: {error:#}", path.name));
                continue;
            }
        };
        let exit = match output.exit_status {
            Some(status) => status.to_string(),
            None => "no exit status".to_string(),
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(receipt) if receipt["state"] == HANDED_OVER => return Ok(receipt),
            Ok(receipt) => failures.push(format!(
                "{}: exit {exit}, answered {receipt}: {}",
                path.name,
                stderr.trim()
            )),
            Err(error) => failures.push(format!(
                "{}: exit {exit}, no receipt ({error}): {}",
                path.name,
                stderr.trim()
            )),
        }
    }
    if failures.is_empty() {
        return Err(format!("{} declares no SSH connection path", target.name));
    }
    Err(failures.join("; "))
}
