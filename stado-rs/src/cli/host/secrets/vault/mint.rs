//! `stado credentials token mint`: a bounded bearer minted, or an existing
//! owner-vault field registered, for one consumer on the vault owner.

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::release_component;
use crate::cli::host::secrets::vault::mirror::read::remote_skarbiec_json_at;
use crate::cli::host::secrets::vault::vault_word;

/// The field of a `--store-item` item that holds the bearer.
const STORED_FIELD: &str = "token";

/// Mint a bounded bearer, or register an existing owner-vault field on TARGET.
///
/// Existing-bearer bytes stay on the target and never enter argv or the report.
/// `raw_token` exposes only a newly generated bearer for a secret-store pipe;
/// `token_file_name` instead persists and reuses it on the target;
/// `store_item` stores a fresh bearer in that owner-vault item's `token`
/// field when the item is absent, then registers the item's value.
#[allow(clippy::too_many_arguments)]
pub async fn vault_token_mint(
    target: &str,
    consumer: &str,
    capabilities: &str,
    audience: &str,
    ttl_seconds: u64,
    replace_capabilities: bool,
    token_item: Option<&str>,
    token_field: &str,
    raw_token: bool,
    token_file_name: Option<&str>,
    store_item: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    vault_word("audience", audience)?;
    if let Some(item) = token_item {
        vault_word("token item", item)?;
        vault_word("token field", token_field)?;
        if raw_token {
            return Err(CmdError::usage(
                "--raw-token cannot be used with an existing --token-item",
            ));
        }
    }
    if raw_token && json_output {
        return Err(CmdError::usage(
            "--raw-token and --json cannot be used together",
        ));
    }
    if let Some(name) = token_file_name {
        release_component("token file name", name)?;
        if raw_token {
            return Err(CmdError::usage(
                "--raw-token and --token-file-name cannot be used together",
            ));
        }
    }
    if let Some(item) = store_item {
        vault_word("store item", item)?;
        if token_item.is_some() || raw_token || token_file_name.is_some() {
            return Err(CmdError::usage(
                "--store-item stores a newly minted bearer; it cannot be used with --token-item, --raw-token or --token-file-name",
            ));
        }
    }
    if capabilities.is_empty()
        || !capabilities
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/,:#".contains(&byte))
    {
        return Err(CmdError::usage(
            "capabilities must be a comma-separated list of exact action:item[#field] values",
        ));
    }
    let mut arguments = vec![
        String::from("grant"),
        String::from("issue"),
        consumer.to_string(),
        String::from("--capabilities"),
        capabilities.to_string(),
        String::from("--audience"),
        audience.to_string(),
        String::from("--ttl-seconds"),
        ttl_seconds.to_string(),
    ];
    if replace_capabilities {
        arguments.push(String::from("--replace-capabilities"));
    }
    // --store-item creates the item on the owner only when it is absent
    // (`set-json --if-absent`, one vault generation, so of two concurrent runs
    // only one writes) and then registers whatever value the item holds. A
    // failed, repeated or concurrent run therefore never leaves a registered
    // bearer that no item holds, nor overwrites a bearer already registered.
    let mut stored = None;
    if let Some(item) = store_item {
        // The canonical item envelope Skarbiec's `set-json` accepts, the same
        // one the release publisher writes for its own bearer.
        let payload = json!({
            "schema": "skarbiec.item.v2",
            "kind": "token",
            "fields": { "token": crate::cli::release_catalog::fresh_bearer() },
            "context": { "consumer": consumer, "audience": audience },
        })
        .to_string();
        let report =
            crate::cli::host::write_vault_item(target, item, "token", &payload, true, None).await?;
        if report["created"].as_bool() == Some(true) {
            stored = Some(report);
        }
    }
    let token_source = token_item
        .map(|item| (item, token_field))
        .or(store_item.map(|item| (item, STORED_FIELD)));
    let (resolved, mut report) =
        remote_skarbiec_json_at(target, &arguments, None, token_source, token_file_name).await?;
    if token_source.is_none() && token_file_name.is_none() {
        let token = report
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: Skarbiec grant issue returned no bearer",
                    resolved.name
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        if raw_token {
            println!("{token}");
            return Ok(());
        }
    }
    if let Some(object) = report.as_object_mut() {
        object.remove("token");
    }
    if json_output {
        let mut metadata = json!({
            "target": resolved.name,
            "status": if token_source.is_some() { "token_registered" } else { "token_minted" },
            "skarbiec": report,
        });
        if let Some((item, field)) = token_source {
            metadata["token_source"] = json!({ "item": item, "field": field });
        }
        if let Some(stored) = &stored {
            metadata["stored_item"] = stored.clone();
        }
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        let operation = if token_source.is_some() {
            "registered"
        } else {
            "minted"
        };
        println!(
            "{}: token {operation} for {consumer} with audience {audience}",
            resolved.name
        );
        if let Some(path) = report.get("token_file").and_then(Value::as_str) {
            println!("Bearer file: {path}");
        }
        if let Some(stored) = &stored {
            println!(
                "Stored as {}#token (revision {} -> {})",
                stored["item"].as_str().unwrap_or_default(),
                stored["before"]["revision"].as_str().unwrap_or_default(),
                stored["after"]["revision"].as_str().unwrap_or_default(),
            );
        }
    }
    Ok(())
}
