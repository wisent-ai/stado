//! `stado credentials grant show`: what one consumer's grant on a host holds,
//! and whether its token file is the bearer the vault recorded.

use serde_json::{json, Value};

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::host::secrets::vault::vault_word;
use crate::cli::CmdError;

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
    let (resolved, listing) =
        remote_skarbiec_json(target, &[String::from("grant"), String::from("list")]).await?;
    let grant = listing
        .as_array()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: Skarbiec did not answer its token list as an array",
                resolved.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
        .iter()
        .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer));
    let Some(grant) = grant else {
        return Err(CmdError::refused(format!(
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
                .map_err(CmdError::from)?;
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
