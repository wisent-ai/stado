use serde_json::json;

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::mirror::{remote_skarbiec_json, skarbiec_tool_path};
use crate::cli::host::secrets::vault::vault_word;

mod show;
pub use show::grant_show;

/// Authorize one consumer to read one field of the item that plays ROLE in
/// TARGET's vault.
///
/// Two reads reach that field, and the grant covers both. A role read
/// (`stado credentials get --role … --route`, Oko's model router) asks for
/// `role:<role>`, and Skarbiec matches that coordinate exactly before it
/// resolves the role, so the grant names the role, `read:role:<role>#<field>`,
/// and keeps working when the role moves to another item. A reader that lists
/// what it may read and then names the item (the workload agent) needs the
/// item that plays the role now, `read:<item>#<field>`. The role must have
/// exactly one live item, so nothing is granted for a role nothing plays.
/// Widening what a unit or a release job may read is a write into the
/// *host's* vault, not into this laptop's. The bearer never enters an
/// argument vector: the consumer's existing token file on the target is
/// named, and Skarbiec reads it there.
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
    ensure_item_read(target, consumer, &item, field, token_file).await?;
    let (host, bearer_path) =
        ensure_item_read(target, consumer, &format!("role:{role}"), field, token_file).await?;
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
        .map_err(CmdError::from)?;
    if !granted.ok() {
        return Err(CmdError::refused(format!(
            "{}: Skarbiec refused to grant {consumer} a read of {item}#{field}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&granted, "remote command failed")
        )));
    }
    Ok((resolved.name, bearer_path))
}
