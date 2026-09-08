use std::io::Read;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::item::read_vault_phase;
use crate::cli::host::secrets::vault::mirror::skarbiec_tool_path;
use crate::cli::host::secrets::vault::vault_word;

/// Store one canonical credential item in TARGET's owner vault.
///
/// The payload is accepted only on stdin and remains stdin across the host
/// channel. The command reports encrypted-record metadata before and after the
/// write; it never decrypts the value for reporting and never rewrites the
/// surrounding vault.
pub async fn vault_item_put(
    target: &str,
    item: &str,
    item_type: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    vault_word("credential type", item_type)?;

    let mut payload = String::new();
    std::io::stdin().lock().read_to_string(&mut payload)?;
    if payload.is_empty() || payload.len() > usize::from(u16::MAX) {
        return Err(CmdError::usage(
            "vault item payload must contain between one and 65535 bytes",
        ));
    }
    let document: Value = serde_json::from_str(&payload).map_err(|error| {
        CmdError::usage(format!("vault item payload is not valid JSON: {error}"))
    })?;
    let payload_type = document
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage("vault item payload requires a string kind"))?;
    if payload_type != item_type {
        return Err(CmdError::usage(format!(
            "vault item payload kind {payload_type:?} does not match --type {item_type:?}"
        )));
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
    let invocation = [
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
        "set-json",
        item,
        "--type",
        item_type,
    ];

    let before = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(CmdError::click)?;
    let stored = crate::deploy::host_channel::run_program_with_stdin(
        &resolved,
        &invocation,
        &payload,
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !stored.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec set-json failed for {item}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&stored, "remote command failed")
        )));
    }
    let after = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(CmdError::click)?;
    if after.state != "active" || after.revision == before.revision {
        return Err(CmdError::click(format!(
            "{}: {item} write was not visible in the encrypted vault",
            resolved.name
        )));
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "kind": item_type,
                "before": {
                    "state": before.state,
                    "revision": before.revision,
                },
                "after": {
                    "state": after.state,
                    "revision": after.revision,
                },
            }))?
        );
    } else {
        println!(
            "{}: stored {item} as {item_type}; state {} -> {}, revision {} -> {}",
            resolved.name, before.state, after.state, before.revision, after.revision
        );
    }
    Ok(())
}
