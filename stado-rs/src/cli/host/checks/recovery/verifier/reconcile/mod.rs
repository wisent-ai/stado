//! Preserve an isolated verifier bearer while making its capabilities
//! match config.

pub(in crate::cli::host) mod shadow;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::recovery::verifier::release::remote_skarbiec_metadata;
use crate::cli::host::secrets::vault::vault_word;

/// Preserve an isolated verifier bearer while making its capabilities match config.
#[allow(clippy::too_many_arguments)]
pub(super) async fn reconcile_verifier(
    target: &str,
    kind: &str,
    config_name: &str,
    consumer: &str,
    token_file_env: &str,
    token_file_default: &str,
    items: std::collections::BTreeSet<String>,
    replace_capabilities: bool,
) -> Result<Value, CmdError> {
    if items.is_empty() {
        return Err(CmdError::click(format!(
            "{config_name} is empty; refusing to mint an unusable verifier grant"
        )));
    }
    for item in &items {
        vault_word(&format!("{kind} verifier item"), item)?;
    }

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let environment_command = format!(
        "printf '%s\\n%s\\n%s\\n' \"${{SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}}\" \
         \"${{GNUPGHOME:-$HOME/.gnupg}}\" \
         \"${{{token_file_env}:-$HOME/.stado/{token_file_default}}}\""
    );
    let environment =
        crate::deploy::host_channel::run_command(&resolved, &environment_command, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        )));
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
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            return Err(CmdError::click(format!(
                "{}: no {label} at {path}",
                resolved.name
            )));
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
    .map_err(|error| CmdError::click(error.to_string()))?;

    let token_metadata =
        remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "tokens")
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
        })?;
    let expires_at = grant
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| CmdError::click(format!("{kind} verifier grant has no numeric expiry")))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CmdError::click(error.to_string()))?
        .as_secs();
    let ttl = expires_at
        .checked_sub(now)
        .filter(|ttl| *ttl > 0)
        .ok_or_else(|| CmdError::click(format!("{kind} verifier grant is already expired")))?;
    // Release publisher items and the route-scoped host-health bearer remain
    // authoritative in the control-plane vault. Their consumers read
    // target-local shadows with the same ids. Atomically replace only those
    // shadows; this copies the current value without rotating or reclassifying
    // the authoritative source.
    let mut source_lifecycles = Vec::new();
    if matches!(kind, "release" | "object") {
        let authoritative_vault = crate::credential_store::owner::vault()
            .map_err(|error| CmdError::click(error.to_string()))?;
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
            .ok_or_else(|| CmdError::click("target vault has no owner identity"))?;
        let target_items =
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "list")
                .await?;
        for item in &items {
            if kind == "object" && item.as_str() != crate::config::HOST_HEALTH_API_ITEM {
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
    let capabilities = items
        .iter()
        .map(|item| format!("read:{item}#token"))
        .collect::<Vec<_>>()
        .join(",");
    let common = format!(
        "set -eu; \
         PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin; export PATH; \
         GNUPGHOME={}; export GNUPGHOME; \
         SKARBIEC_VAULT_FILE={}; export SKARBIEC_VAULT_FILE; \
         token_file={}; staged=''; \
         if [ -L \"$token_file\" ]; then exit 40; fi",
        crate::deploy::shlex_quote(&gnupg_home),
        crate::deploy::shlex_quote(&vault),
        crate::deploy::shlex_quote(&token_file),
    );
    let command = if replace_capabilities {
        format!(
            "{common}; \
             if [ -f \"$token_file\" ]; then source_file=\"$token_file\"; \
             else \
               staged=\"$token_file.stado-new.$$\"; \
               trap '/bin/rm -f \"$staged\"' EXIT HUP INT TERM; \
               umask 077; /usr/bin/openssl rand -hex 32 > \"$staged\"; \
               source_file=\"$staged\"; \
             fi; \
             {} token-mint {} --capabilities {} --replace-capabilities \
               --token-file \"$source_file\" --ttl-seconds {} > /dev/null; \
             if [ -n \"$staged\" ]; then /bin/mv -f \"$staged\" \"$token_file\"; trap - EXIT HUP INT TERM; fi",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(&capabilities),
            ttl,
        )
    } else {
        let item = items
            .first()
            .expect("product-scoped release verifier has one item");
        format!(
            "{common}; \
             if [ -f \"$token_file\" ]; then \
               {} token-ensure-read {} {} --field token --token-file \"$token_file\" > /dev/null; \
             else \
               staged=\"$token_file.stado-new.$$\"; \
               trap '/bin/rm -f \"$staged\"' EXIT HUP INT TERM; \
               umask 077; /usr/bin/openssl rand -hex 32 > \"$staged\"; \
               {} token-mint {} --capabilities {} --replace-capabilities \
                 --token-file \"$staged\" --ttl-seconds {} > /dev/null; \
               /bin/mv -f \"$staged\" \"$token_file\"; trap - EXIT HUP INT TERM; \
             fi",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(&capabilities),
            ttl,
        )
    };
    let reconciled = crate::deploy::host_channel::run_command(&resolved, &command, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !reconciled.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier reconciliation failed without replacing its token file: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&reconciled, "remote command failed")
        )));
    }

    let item_list = items.iter().cloned().collect::<Vec<_>>();
    let report = json!({
        "target": resolved.name,
        "kind": kind,
        "consumer": consumer,
        "items": item_list,
        "source_lifecycles": source_lifecycles,
        "bearer_preserved": bearer_preserved,
        "expires_at": expires_at,
        "exact": replace_capabilities,
    });
    Ok(report)
}
