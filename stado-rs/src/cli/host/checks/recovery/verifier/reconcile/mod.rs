//! Reconcile the reads of declared roles on Stado's grant without replacing
//! unrelated capabilities.

pub(in crate::cli::host) mod shadow;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::recovery::verifier::release::remote_skarbiec_metadata;
use crate::cli::host::secrets::vault::vault_word;

/// Ensure the declared roles read on Stado's grant, preserving other scopes.
/// `items` holds roles; each is translated to the item playing it in the
/// vault that holds the authoritative copy, and only that id is granted.
pub(super) async fn reconcile_verifier(
    target: &str,
    kind: &str,
    config_name: &str,
    consumer: &str,
    token_file_env: &str,
    token_file_default: &str,
    items: std::collections::BTreeSet<String>,
) -> Result<Value, CmdError> {
    if items.is_empty() {
        return Err(CmdError::click(format!(
            "{config_name} is empty; refusing to mint an unusable verifier grant"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    for role in &items {
        vault_word(&format!("{kind} verifier role"), role)?;
    }

    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(CmdError::from)?;
    let environment_command = format!(
        "printf '%s\\n%s\\n%s\\n' \"${{SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}}\" \
         \"${{GNUPGHOME:-$HOME/.gnupg}}\" \
         \"${{{token_file_env}:-$HOME/.stado/{token_file_default}}}\""
    );
    let environment =
        crate::deploy::host_channel::run_command(&resolved, &environment_command, &runner)
            .await
            .map_err(CmdError::from)?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables.next().unwrap_or_default().to_string();
    let gnupg_home = variables.next().unwrap_or_default().to_string();
    let token_file = variables.next().unwrap_or_default().to_string();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    for (label, path, test) in [
        ("Skarbiec binary", skarbiec.as_str(), "-x"),
        ("vault", vault.as_str(), "-f"),
    ] {
        let present = crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("{test} {}", crate::deploy::shlex_quote(path)),
            &runner,
        )
        .await
        .map_err(CmdError::from)?;
        if !present {
            return Err(
                CmdError::click(format!("{}: no {label} at {path}", resolved.name))
                    .stating(crate::primitives::failure::FailureCode::NotFound),
            );
        }
    }
    let bearer_preserved = crate::deploy::host_channel::remote_test(
        &resolved,
        &format!(
            "-f {} && ! -L {}",
            crate::deploy::shlex_quote(&token_file),
            crate::deploy::shlex_quote(&token_file),
        ),
        &runner,
    )
    .await
    .map_err(CmdError::from)?;

    let token_metadata =
        remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "grants")
            .await?;
    let grant = token_metadata
        .as_array()
        .and_then(|tokens| {
            tokens
                .iter()
                .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer))
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: {consumer} has no existing grant",
                resolved.name
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let expires_at = grant
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CmdError::click(format!("{kind} verifier grant has no numeric expiry"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    let now = crate::cli::entry::error::epoch_seconds()?;
    if expires_at <= now {
        return Err(CmdError::refused(format!(
            "{kind} verifier grant is already expired"
        )));
    }
    // Each role is played by an item in the vault that holds the
    // authoritative copy: the control-plane vault for release publishers and
    // the route-scoped host-health bearer, the target's own vault otherwise.
    let listed = |value: Value| -> Result<Vec<crate::skarbiec::ItemInfo>, CmdError> {
        serde_json::from_value(value).map_err(|error| {
            CmdError::click(format!(
                "{}: Skarbiec list did not answer items: {error}",
                resolved.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })
    };
    // The control-plane vault is the owner vault on this host: it is listed
    // with the installed Skarbiec, the read every owner-path call uses, not
    // through a per-host launcher script nothing installs (960e3bb9).
    let authority = if matches!(kind, "release" | "object") {
        crate::credential_store::owner::list_items().map_err(CmdError::from)?
    } else {
        listed(
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "list")
                .await?,
        )?
    };
    let mut played = Vec::new();
    for role in &items {
        let id = crate::skarbiec::roles::item_for_role(&authority, role)
            .map_err(|refusal| {
                CmdError::refused(format!("{}: {kind} verifier: {refusal}", resolved.name))
            })?
            .id
            .clone();
        played.push((role.clone(), id));
    }
    // Their consumers read target-local shadows with the same ids. Atomically
    // replace only those shadows; this copies the current value without
    // rotating or reclassifying the authoritative source.
    let mut source_lifecycles = Vec::new();
    if matches!(kind, "release" | "object") {
        let authoritative_vault =
            crate::credential_store::owner::vault().map_err(CmdError::from)?;
        let authoritative_text = std::fs::read_to_string(&authoritative_vault)?;
        let authoritative: Value = serde_json::from_str(&authoritative_text)?;
        let target_vaults =
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "vaults")
                .await?;
        let target_owner = target_vaults
            .get("vaults")
            .and_then(Value::as_array)
            .and_then(|vaults| {
                vaults
                    .iter()
                    .find(|entry| entry.get("path").and_then(Value::as_str) == Some(vault.as_str()))
            })
            .and_then(|entry| entry.get("owner"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::click("target vault has no owner identity")
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        let target_items =
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "list")
                .await?;
        for (role, item) in &played {
            if kind == "object" && role.as_str() != crate::config::HOST_HEALTH_API_ROLE {
                continue;
            }

            shadow::converge_item(
                &mut source_lifecycles,
                &resolved,
                &runner,
                kind,
                item,
                &skarbiec,
                &vault,
                &gnupg_home,
                &authoritative,
                &target_items,
                target_owner,
            )
            .await?;
        }
    }
    let common = format!(
        "set -eu; \
         PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin; export PATH; \
         GNUPGHOME={}; export GNUPGHOME; \
         SKARBIEC_VAULT_FILE={}; export SKARBIEC_VAULT_FILE; \
         token_file={}; \
         if [ ! -f \"$token_file\" ] || [ -L \"$token_file\" ]; then \
           printf '%s\\n' 'Stado grant file is missing or a symlink' >&2; exit 40; \
         fi",
        crate::deploy::shlex_quote(&gnupg_home),
        crate::deploy::shlex_quote(&vault),
        crate::deploy::shlex_quote(&token_file),
    );
    let mut command = common;
    for (_, item) in &played {
        command.push_str(&format!(
            "; {} grant ensure {} {} --field token --token-file \"$token_file\" > /dev/null",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(item),
        ));
    }
    let reconciled = crate::deploy::host_channel::run_command(&resolved, &command, &runner)
        .await
        .map_err(CmdError::from)?;
    if !reconciled.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} item grant reconciliation failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&reconciled, "remote command failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let report = json!({
        "target": resolved.name,
        "kind": kind,
        "consumer": consumer,
        "items": items,
        "source_lifecycles": source_lifecycles,
        "bearer_preserved": bearer_preserved,
        "expires_at": expires_at,
        "exact": false,
    });
    Ok(report)
}
