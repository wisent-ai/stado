use std::io::Read;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::item::read_vault_phase;
use crate::cli::host::secrets::vault::mirror::{remote_skarbiec_json, skarbiec_tool_path};
use crate::cli::host::secrets::vault::vault_word;
use crate::skarbiec::{roles, ItemInfo};

/// Store the secret that plays ROLE in TARGET's owner vault, from stdin.
///
/// The caller names what the secret is for, never an item (see
/// [`write_role_item`]). The payload is accepted only on stdin and remains
/// stdin across the host channel; the report carries encrypted-record
/// metadata only.
pub async fn vault_item_put(
    target: &str,
    role: &str,
    item_type: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let mut payload = String::new();
    std::io::stdin().lock().read_to_string(&mut payload)?;
    let report = write_role_item(target, role, item_type, &payload, false).await?;
    print_report(&report, json_output)
}

/// Write the item that plays ROLE in TARGET's vault: the one live item
/// carrying `stado:role:<role>` is rotated in place — or, with
/// `keep_existing`, left as it is and reported `created: false` — and when
/// none does a new item is created under a random id with that tag. Two
/// items in one role are refused, because choosing between them would be a
/// guess.
pub(crate) async fn write_role_item(
    target: &str,
    role: &str,
    item_type: &str,
    payload: &str,
    keep_existing: bool,
) -> Result<Value, CmdError> {
    vault_word("role", role)?;
    let (_, listing) = remote_skarbiec_json(target, &["list".into()]).await?;
    let items: Vec<ItemInfo> = serde_json::from_value(listing).map_err(|error| {
        CmdError::click(format!(
            "{target}: Skarbiec list did not answer items: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let tag = roles::role_tag(role);
    let mut report = match roles::holders(&items, role).as_slice() {
        [] => {
            let item = roles::fresh_item_id();
            write_vault_item(target, &item, item_type, payload, false, Some(&tag)).await?
        }
        [_] if keep_existing => json!({ "created": false, "target": target, "kind": item_type }),
        [one] => write_vault_item(target, &one.id, item_type, payload, false, None).await?,
        several => {
            return Err(CmdError::refused(format!(
                "{target}: {} items carry {tag}; exactly one item may play role {role}",
                several.len()
            )))
        }
    };
    report["role"] = Value::from(role);
    Ok(report)
}

/// Keep the item named ITEM as the one item playing ROLE in TARGET's vault,
/// minting it from PAYLOAD when the vault holds neither. A declaration that
/// names its item needs both: a release publisher is read as the item named
/// after its product, while role reads and grants ask for the role. Minting
/// under a random id satisfied only the role, so the declaration guard then
/// refused the publisher just minted (6ca130f5). A lone holder under another
/// id is renamed ITEM, keeping its bearer and history; an ITEM without the
/// role tag gains it beside its other tags. ITEM held while another item
/// plays the role is refused: which bearer readers should get is a guess.
pub(crate) async fn write_named_role_item(
    target: &str,
    item: &str,
    role: &str,
    item_type: &str,
    payload: &str,
) -> Result<Value, CmdError> {
    vault_word("vault item", item)?;
    vault_word("role", role)?;
    let (_, listing) = remote_skarbiec_json(target, &["list".into()]).await?;
    let items: Vec<ItemInfo> = serde_json::from_value(listing).map_err(|error| {
        CmdError::click(format!(
            "{target}: Skarbiec list did not answer items: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let tag = roles::role_tag(role);
    let named = items
        .iter()
        .find(|candidate| candidate.id == item && candidate.deleted != Some(true));
    match (roles::holders(&items, role).as_slice(), named) {
        ([one], _) if one.id == item => {
            Ok(json!({ "created": false, "target": target, "item": item, "kind": item_type }))
        }
        ([], None) => write_vault_item(target, item, item_type, payload, true, Some(&tag)).await,
        ([one], None) => {
            let moved = super::change::rename::move_vault_item(target, &one.id, item).await?;
            Ok(json!({
                "created": false, "target": target, "item": item, "renamed": moved.report(),
            }))
        }
        ([], Some(untagged)) => {
            let tags = untagged
                .tags
                .iter()
                .flatten()
                .map(String::as_str)
                .chain([tag.as_str()])
                .collect::<Vec<_>>()
                .join(",");
            let host = credential_host(target).await?;
            let runner = crate::deploy::production_runner();
            let skarbiec =
                crate::cli::host::release_managed_skarbiec(&host.target, &runner, &host.home)
                    .await?;
            super::change::retag::run_retag(
                &host.target,
                &host.gnupg_home,
                &host.vault,
                &skarbiec,
                item,
                &tags,
                &runner,
            )
            .await?
            .map_err(|detail| {
                CmdError::refused(format!("{target}: {item} could not take {tag}: {detail}"))
            })?;
            Ok(json!({ "created": false, "target": target, "item": item, "tagged": tag }))
        }
        (holders, _) => Err(CmdError::refused(format!(
            "{target}: {} plays role {role} while release readers ask for item {item}; exactly one \
             item may hold it, and it must be {item}. Rename or retag the others with `stado \
             credentials item rename|retag --host {target}` and declare again",
            holders
                .iter()
                .map(|holder| holder.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn print_report(report: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let subject = report["role"]
        .as_str()
        .map(|role| format!("role {role}"))
        .unwrap_or_else(|| report["item"].as_str().unwrap_or_default().to_string());
    println!(
        "{}: stored {subject} as {}; state {} -> {}, revision {} -> {}",
        report["target"].as_str().unwrap_or_default(),
        report["kind"].as_str().unwrap_or_default(),
        report["before"]["state"].as_str().unwrap_or_default(),
        report["after"]["state"].as_str().unwrap_or_default(),
        report["before"]["revision"].as_str().unwrap_or_default(),
        report["after"]["revision"].as_str().unwrap_or_default(),
    );
    Ok(())
}

/// The write itself, for a caller that composed the payload in memory: the
/// publisher declaration mints its bearer this way, so the secret never
/// touches a shell or an argument vector on its way to the host.
pub(crate) async fn store_vault_item(
    target: &str,
    item: &str,
    item_type: &str,
    payload: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = write_vault_item(target, item, item_type, payload, false, None).await?;
    print_report(&report, json_output)
}

/// Write one item and return its encrypted-record report (target, item, kind,
/// before and after state and revision) instead of printing it, for a caller
/// whose own output is a single JSON document.
///
/// With `if_absent` the owner's Skarbiec creates the item only when no live
/// item has that id (`set-json --if-absent`, decided and written under one
/// vault generation, so of two concurrent creators only one writes); an
/// existing item is left as it is and reported with `created: false`.
///
/// `tags` sets the item's tag list (`set-json --tags`); `None` leaves an
/// existing item's tags as they are.
pub(crate) async fn write_vault_item(
    target: &str,
    item: &str,
    item_type: &str,
    payload: &str,
    if_absent: bool,
    tags: Option<&str>,
) -> Result<Value, CmdError> {
    vault_word("vault item", item)?;
    vault_word("credential type", item_type)?;
    if payload.is_empty() || payload.len() > usize::from(u16::MAX) {
        return Err(CmdError::usage(
            "vault item payload must contain between one and 65535 bytes",
        ));
    }
    let document: Value = serde_json::from_str(payload).map_err(|error| {
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
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;
    let tool_path = skarbiec_tool_path(&home);
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let mut invocation = vec![
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
    if if_absent {
        invocation.push("--if-absent");
    }
    if let Some(tags) = tags {
        invocation.push("--tags");
        invocation.push(tags);
    }

    let before = read_vault_phase(&resolved, &vault, item, &runner).await?;
    let stored = crate::deploy::host_channel::run_program_with_stdin(
        &resolved,
        &invocation,
        payload,
        &runner,
    )
    .await
    .map_err(CmdError::from)?;
    if !stored.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec set-json failed for {item}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&stored, "remote command failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // A Skarbiec older than `--if-absent` answers without `created`; its
    // write is taken as a creation, which is what it did.
    let created = serde_json::from_str::<Value>(&stored.stdout)
        .ok()
        .and_then(|answer| answer.get("created").and_then(Value::as_bool))
        .unwrap_or(true);
    let after = read_vault_phase(&resolved, &vault, item, &runner).await?;
    if created && (after.state != "active" || after.revision == before.revision) {
        return Err(CmdError::click(format!(
            "{}: {item} write was not visible in the encrypted vault",
            resolved.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }

    Ok(json!({
        "created": created,
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
    }))
}
