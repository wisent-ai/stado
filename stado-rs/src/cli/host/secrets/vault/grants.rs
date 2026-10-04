use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::mirror::{remote_skarbiec_json, skarbiec_tool_path};
use crate::cli::host::secrets::vault::vault_word;

/// Authorize one consumer to read one field of the item that plays ROLE in
/// TARGET's vault.
///
/// A Skarbiec grant is per item and per field, so the role is translated to
/// the one item carrying `stado:role:<role>` in that vault at the moment of
/// granting; no caller names the item. Widening what a unit or a release job
/// may read is a write into the *host's* vault, not into this laptop's. The
/// bearer never enters an argument vector: the consumer's existing token file
/// on the target is named, and Skarbiec reads it there.
pub async fn grant_item_read(
    target: &str,
    consumer: &str,
    role: &str,
    field: &str,
    token_file: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("role", role)?;
    let (_, listing) = remote_skarbiec_json(target, &["list".into()]).await?;
    let items: Vec<crate::skarbiec::ItemInfo> =
        serde_json::from_value(listing).map_err(|error| {
            CmdError::click(format!(
                "{target}: Skarbiec list did not answer items: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    let item = crate::skarbiec::roles::item_for_role(&items, role)
        .map_err(|refusal| CmdError::refused(format!("{target}: {refusal}")))?
        .id
        .clone();
    let (host, bearer_path) = ensure_item_read(target, consumer, &item, field, token_file).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": host,
                "consumer": consumer,
                "role": role,
                "field": field,
                "token_file": bearer_path,
                "granted": true,
            }))?
        );
    } else {
        println!("{host}: {consumer} may read role {role}#{field}");
    }
    Ok(())
}

/// Make `fields` of `item` readable by the consumer this host's own credential
/// reads authenticate as, in the vault those reads reach.
///
/// On the vault owner that is its own vault, widened in place. Anywhere else
/// the host reads the owner's vault through the broker, and its vault files
/// are retired copies: widening one of them refuses every command that needs
/// a grant first — `stado fleet key generate`, `stado dns list` — so the
/// grant goes to the owner over the host channel. Progress goes to stderr: callers print one
/// JSON document on stdout.
pub async fn settle_consumer_reads(item: &str, fields: &[&str]) -> Result<(), CmdError> {
    if crate::credential_store::skarbiec_url().is_none() {
        return Ok(());
    }
    let (owner, here) = crate::cli::release_catalog::fleet_hosts().await?;
    if owner == here {
        let outcome =
            crate::credential_store::grant::settle_field_reads(item, fields).map_err(|error| {
                CmdError::click(format!("cannot make {item} readable: {error}"))
                    .stating(error.failure_code())
            })?;
        if let Some(outcome) = outcome.filter(crate::credential_store::grant::GrantOutcome::wrote) {
            eprintln!(
                "granted read on {} ({} capabilities held, was {})",
                outcome.added.join(", "),
                outcome.held_after,
                outcome.held_before
            );
        }
        return Ok(());
    }
    let consumer = crate::config::skarbiec_consumer();
    let token_file =
        crate::cli::release_catalog::home_relative(crate::config::skarbiec_token_file());
    for field in fields {
        let (host, _) = ensure_item_read(&owner, consumer, item, field, &token_file)
            .await
            .map_err(|error| {
                let mut widened = CmdError::click(format!(
                    "{item}#{field} could not be made readable by {consumer} on the vault owner \
                     {owner}: {}",
                    error.message.as_deref().unwrap_or("no detail")
                ));
                widened.failure = error.failure;
                widened
            })?;
        eprintln!("{host}: {consumer} may read {item}#{field}");
    }
    Ok(())
}

/// Add a read of `item#field` to `consumer`'s grant on the vault owner, keeping
/// the bearer in `token_file` there — a declared file, `$HOME/…`, `~/…` or
/// absolute on the owner — and every capability the grant holds. Answers the
/// owner's name.
pub async fn ensure_declared_read(
    consumer: &str,
    item: &str,
    field: &str,
    token_file: &str,
) -> Result<String, CmdError> {
    let (owner, _) = crate::cli::release_catalog::fleet_hosts().await?;
    let token_file = match token_file.strip_prefix("$HOME/") {
        Some(rest) => format!("~/{rest}"),
        None => token_file.to_string(),
    };
    ensure_item_read(&owner, consumer, item, field, &token_file)
        .await
        .map(|(host, _)| host)
}

/// The grant itself, printing nothing: the resolved host name and the bearer
/// path Skarbiec read on it.
async fn ensure_item_read(
    target: &str,
    consumer: &str,
    item: &str,
    field: &str,
    token_file: &str,
) -> Result<(String, String), CmdError> {
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
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;
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
        return Err(CmdError::refused(format!(
            "{}: Skarbiec refused to grant {consumer} a read of {item}#{field}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&granted, "remote command failed")
        )));
    }
    Ok((resolved.name, bearer_path))
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
