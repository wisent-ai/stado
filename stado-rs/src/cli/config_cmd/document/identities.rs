//! Atomically retire role-specific Skarbiec settings after the product grant
//! has been consolidated. A refused migration leaves the original untouched.

use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::config_file;

const RETIRED: &[&str] = &[
    "credentials.admin",
    "alerts.skarbiec",
    "object_api.skarbiec",
    "release_api.skarbiec",
    "release.publisher_skarbiec",
    "machine_api.skarbiec",
    "service_api.skarbiec",
    "rate_limit.skarbiec",
    "integration.skarbiec",
    "integration.provider_skarbiec",
    "backend.push_skarbiec",
    "backend.messaging.skarbiec.url",
    "backend.messaging.skarbiec.consumer",
    "backend.messaging.skarbiec.token_file",
    "backend.messaging.skarbiec.token",
    "agent.skarbiec.token",
];

fn remove(root: &mut Map<String, Value>, path: &str) -> Option<Value> {
    let (head, rest) = path.split_once('.')?;
    let nested = root.get_mut(head)?.as_object_mut()?;
    if rest.contains('.') {
        remove(nested, rest)
    } else {
        nested.remove(rest)
    }
}

pub(in crate::cli::config_cmd) fn migrate_identities() -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists to migrate"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&original)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("config file must contain a JSON object"))?;
    let mut removed = Vec::new();
    for key in RETIRED {
        if remove(root, key).is_some() {
            removed.push(*key);
        }
    }
    // A product is its own identity: its publisher and deployer entries name
    // the item called after the product, and a deployer no longer carries a
    // consumer of its own. `stado credentials item rename` moves the vault
    // items to the same names.
    for (section, entries) in [("release_api", "publishers"), ("service_api", "deployers")] {
        let Some(table) = root
            .get_mut(section)
            .and_then(|value| value.get_mut(entries))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (product, entry) in table.iter_mut() {
            let Some(entry) = entry.as_object_mut() else {
                continue;
            };
            if entry.get("item").and_then(Value::as_str) != Some(product.as_str()) {
                entry.insert("item".into(), Value::from(product.as_str()));
                removed.push("a role-named product item");
            }
            if entry.remove("consumer").is_some() {
                removed.push("a product deployer consumer");
            }
        }
    }
    // An integration provider reads its items as Stado too.
    if let Some(providers) = root
        .get_mut("integration")
        .and_then(|value| value.get_mut("providers"))
        .and_then(Value::as_object_mut)
    {
        for provider in providers.values_mut().filter_map(Value::as_object_mut) {
            for key in ["consumer", "token_file"] {
                if provider.remove(key).is_some() {
                    removed.push("an integration provider identity");
                }
            }
        }
    }
    let secrets = root
        .entry("secrets")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("secrets must be an object"))?;
    let skarbiec = secrets
        .entry("skarbiec")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("secrets.skarbiec must be an object"))?;
    let previous = skarbiec.insert("consumer".into(), Value::from("stado"));
    let token_file = skarbiec.get("token_file").cloned();
    // On a host Stado owns, its workload agent reads as Stado too; a scoped
    // `*-agent` grant stays for rented machines.
    if let Some(agent) = root
        .get_mut("agent")
        .and_then(|value| value.get_mut("skarbiec"))
        .and_then(Value::as_object_mut)
    {
        let retired = agent
            .get("consumer")
            .and_then(Value::as_str)
            .is_some_and(|name| {
                matches!(
                    name,
                    "stado-local-agent" | "stado-azure-agent" | "stado-control-plane"
                )
            });
        if retired {
            agent.insert("consumer".into(), Value::from("stado"));
            if let Some(token_file) = token_file {
                agent.insert("token_file".into(), token_file);
            }
            removed.push("agent.skarbiec.consumer");
        }
    }
    let changed = !removed.is_empty() || previous.as_ref().and_then(Value::as_str) != Some("stado");
    if !changed {
        println!(
            "{}: identity configuration already uses stado",
            path.display()
        );
        return Ok(());
    }
    let problems = config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "identity migration refused; config unchanged: {}",
            problems.join("; ")
        )));
    }
    // A declaration alone is not proof of access. Refuse a cutover to an
    // absent local bearer; grant consolidation verifies the host-side grant.
    let token_file = document
        .pointer("/secrets/skarbiec/token_file")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}/.stado/stado-skarbiec-token",
                std::env::var("HOME").unwrap_or_default()
            )
        });
    let token_file = config_file::expand_tilde(&token_file);
    if !token_file.is_file() {
        return Err(CmdError::click(format!(
            "identity migration refused; {} has no Stado bearer file; config unchanged",
            token_file.display()
        )));
    }
    // Stamped, because the migration runs again each time another key is
    // retired: a fixed name refused the second run with `File exists`.
    let backup = std::path::PathBuf::from(format!(
        "{}.before-identity-migration-{}",
        path.display(),
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    let mut backup_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot preserve config at {}: {error}",
                backup.display()
            ))
        })?;
    std::io::Write::write_all(&mut backup_file, original.as_bytes())?;
    let temporary = std::path::PathBuf::from(format!("{}.migrating-identities", path.display()));
    std::fs::write(
        &temporary,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!(
        "{}: migrated identity settings to stado; removed {}; previous file: {}",
        path.display(),
        removed.join(", "),
        backup.display()
    );
    Ok(())
}
