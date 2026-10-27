//! Revoking a retired grant once the `stado` grant covers it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::CmdError;

/// Revoke a retired consumer's grant on HOST once Stado holds everything it
/// could read, so one identity is left instead of one per role.
///
/// A capability on an item the vault no longer holds (a role-named item that
/// was renamed to its product) opens nothing, so it does not keep the retired
/// grant alive.
///
/// Refused for `stado` itself, and refused while the retired grant still
/// carries a capability the `stado` grant lacks: `consolidate` first, then
/// revoke, so no reader loses access in between.
pub async fn revoke_retired(host: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    if consumer == "stado" {
        return Err(CmdError::usage(
            "stado is the identity that remains; it is never revoked",
        ));
    }
    let (target, listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let rows = listing.as_array().ok_or_else(|| {
        CmdError::click(format!(
            "{}: Skarbiec grant list did not answer an array",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    // A capability without its action or item cannot be compared: rendered
    // with an empty item it named no held item, fell out of the coverage
    // check below and let a revoke through that stado did not cover. Such a
    // grant is refused, and an absent field is rendered absent, not as "#".
    let capabilities_of = |name: &str| -> Result<Option<BTreeSet<String>>, CmdError> {
        let Some(grant) = rows.iter().find(|row| row["consumer"] == name) else {
            return Ok(None);
        };
        let Some(entries) = grant["capabilities"].as_array() else {
            return Ok(None);
        };
        let mut rendered = BTreeSet::new();
        for entry in entries {
            let (Some(action), Some(item)) = (entry["action"].as_str(), entry["item"].as_str())
            else {
                return Err(CmdError::refused(format!(
                    "{}: {name}'s grant lists a capability without its action or item, so its \
                     coverage cannot be judged; nothing was revoked: {entry}",
                    target.name
                )));
            };
            rendered.insert(match entry["field"].as_str() {
                Some(field) => format!("{action}:{item}#{field}"),
                None => format!("{action}:{item}"),
            });
        }
        Ok(Some(rendered))
    };
    let retired = capabilities_of(consumer)?.ok_or_else(|| {
        CmdError::click(format!(
            "{}: no grant for {consumer}; nothing to revoke",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    let stado = capabilities_of("stado")?.ok_or_else(|| {
        CmdError::click(format!(
            "{}: no grant for stado; consolidate before revoking",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::Refused)
    })?;
    let (_, items) = remote_skarbiec_json(host, &["list".into()]).await?;
    // The items the vault holds decide whether revoking loses access, so a
    // list in another shape, or a held row without its id, is refused: read
    // as nothing held, the check found nothing missing and the grant went.
    let unread = |detail: String| {
        CmdError::click(format!("{}: skarbiec list {detail}", target.name))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    let rows = items
        .as_array()
        .ok_or_else(|| unread(format!("answered no list of items: {items}")))?;
    let mut held: BTreeSet<&str> = BTreeSet::new();
    for row in rows {
        if row.get("deleted").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| unread(format!("answered a held item without its id: {row}")))?;
        held.insert(id);
    }
    // A capability is `action:item` or `action:item#field`, as
    // `capabilities_of` spells them; one without `:` is not a capability.
    let item_of = |capability: &str| -> Result<String, CmdError> {
        let (_action, rest) = capability.split_once(':').ok_or_else(|| {
            CmdError::click(format!(
                "{}: {consumer} holds a capability without an action: {capability:?}",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        Ok(match rest.rsplit_once('#') {
            Some((item, _field)) => item.to_string(),
            None => rest.to_string(),
        })
    };
    let mut missing: Vec<&String> = Vec::new();
    for capability in retired.difference(&stado) {
        if held.contains(item_of(capability)?.as_str()) {
            missing.push(capability);
        }
    }
    if !missing.is_empty() {
        return Err(CmdError::refused(format!(
            "{}: stado does not hold {missing:?} that {consumer} can read; run `stado credentials grant consolidate --host {} --from {consumer} --token-file <stado bearer>` first",
            target.name, target.name
        )));
    }
    remote_skarbiec_json(
        host,
        &["grant".into(), "revoke".into(), consumer.to_string()],
    )
    .await?;
    let report = json!({"host": target.name, "revoked": consumer, "covered_by": "stado", "capabilities": retired});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: revoked {consumer}; stado holds all {} of its capabilities",
            target.name,
            retired.len()
        );
    }
    Ok(())
}
