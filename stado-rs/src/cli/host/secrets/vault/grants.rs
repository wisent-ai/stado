use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::mirror::{remote_skarbiec_json, skarbiec_tool_path};
use crate::cli::host::secrets::vault::vault_word;

/// Authorize one consumer to read one field of one item in TARGET's vault.
///
/// A Skarbiec grant is per item and per field, so widening what a unit or a
/// release job may read is a write into the *host's* vault, not into this
/// laptop's. The bearer never enters an argument vector: the consumer's
/// existing token file on the target is named, and Skarbiec reads it there.
pub async fn grant_item_read(
    target: &str,
    consumer: &str,
    item: &str,
    field: &str,
    token_file: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    vault_word("vault item", item)?;
    vault_word("item field", field)?;
    if token_file.trim().is_empty() {
        return Err(CmdError::usage(
            "--token-file must name the consumer's existing bearer file on the target",
        ));
    }

    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let tool_path = skarbiec_tool_path(&home);
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let bearer_path = if token_file.starts_with('/') {
        token_file.to_string()
    } else {
        format!("{home}/{}", token_file.trim_start_matches("~/"))
    };
    let invocation = [
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
        "grant",
        "ensure",
        consumer,
        item,
        "--field",
        field,
        "--token-file",
        bearer_path.as_str(),
    ];
    let granted = crate::deploy::host_channel::run_program(&resolved, &invocation, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !granted.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec refused to grant {consumer} a read of {item}#{field}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&granted, "remote command failed")
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "consumer": consumer,
                "item": item,
                "field": field,
                "token_file": bearer_path,
                "granted": true,
            }))?
        );
    } else {
        println!("{}: {consumer} may read {item}#{field}", resolved.name);
    }
    Ok(())
}

/// Report what one consumer's grant on TARGET actually holds.
///
/// Two facts decide every "not authorized to read item field" refusal, and
/// until now neither could be read: which capabilities the grant records, and
/// whether the bearer in the consumer's token file is the bearer the vault
/// recorded for it. Both are reported here as fields, so the answer to "may
/// this consumer read that" is a measurement rather than a failed attempt.
///
/// The bearer verdict is taken by re-asserting a capability the grant already
/// holds. Skarbiec's `token-ensure-read` compares the presented bearer first
/// and, for a capability already present, records nothing: it takes its
/// `unchanged` branch, writes no vault and appends no audit entry, and then
/// exercises the same two predicates the serving read applies. A grant with no
/// capability has nothing to re-assert, and says so instead of guessing.
pub async fn grant_show(
    target: &str,
    consumer: &str,
    token_file: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    let (resolved, listing) = remote_skarbiec_json(target, &[String::from("tokens")]).await?;
    let grant = listing
        .as_array()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: Skarbiec did not answer its token list as an array",
                resolved.name
            ))
        })?
        .iter()
        .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer));
    let Some(grant) = grant else {
        return Err(CmdError::click(format!(
            "{} declares no grant for {consumer}; add it to the vault declared by secrets.skarbiec.vault_file",
            resolved.name
        )));
    };
    let capabilities: Vec<String> = grant
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|capability| {
                    let item = capability
                        .get("item")
                        .and_then(Value::as_str)
                        .unwrap_or("<no item>");
                    let action = capability
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or("<no action>");
                    match capability.get("field").and_then(Value::as_str) {
                        Some(field) => format!("{item}#{field}:{action}"),
                        None => format!("{item}:{action}"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut bearer_verdict = String::from("not checked: no --token-file was named");
    let mut effective: Option<bool> = None;
    if let Some(token_file) = token_file {
        // Arguments cross the host channel verbatim, with no shell to expand a
        // tilde, so the target's own home resolves the path here.
        let bearer_path = if token_file.starts_with('/') {
            token_file.to_string()
        } else {
            let runner = crate::deploy::production_runner();
            let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            format!("{home}/{}", token_file.trim_start_matches("~/"))
        };
        let probe = grant
            .get("capabilities")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries.iter().find_map(|capability| {
                    if capability.get("action").and_then(Value::as_str) != Some("read") {
                        return None;
                    }
                    let item = capability.get("item").and_then(Value::as_str)?;
                    let field = capability.get("field").and_then(Value::as_str)?;
                    Some((item.to_string(), field.to_string()))
                })
            });
        match probe {
            None => {
                bearer_verdict = String::from(
                    "not checked: the grant records no field read to re-assert without widening it",
                );
            }
            Some((item, field)) => {
                let arguments = [
                    String::from("grant"),
                    String::from("ensure"),
                    consumer.to_string(),
                    item,
                    String::from("--field"),
                    field,
                    String::from("--token-file"),
                    bearer_path.clone(),
                ];
                match remote_skarbiec_json(target, &arguments).await {
                    Ok((_, answer)) => {
                        effective = answer.get("effective").and_then(Value::as_bool);
                        bearer_verdict = match answer.get("refusal").and_then(Value::as_str) {
                            Some(reason) => format!("matches the recorded bearer; read {reason}"),
                            None => String::from("matches the recorded bearer"),
                        };
                    }
                    Err(error) => bearer_verdict = error.to_string(),
                }
            }
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "consumer": consumer,
                "audience": grant.get("audience"),
                "expires_at": grant.get("expires_at"),
                "workload_bound": grant.get("workload_bound"),
                "capabilities": capabilities,
                "token_file": token_file,
                "token_file_match": bearer_verdict,
                "effective": effective,
            }))?
        );
    } else {
        println!("{}: {consumer}", resolved.name);
        if capabilities.is_empty() {
            println!("  (the grant records no capability)");
        }
        for capability in &capabilities {
            println!("  {capability}");
        }
        println!("  token file: {bearer_verdict}");
    }
    Ok(())
}
