//! One vault item: show it, change it, replace it, or stamp what the vault's
//! items hold so duplicates can be found.

pub(in crate::cli::host) mod change;
pub(in crate::cli::host) mod put;
pub(in crate::cli::host) mod show;
pub(in crate::cli::host) mod upgrade;

use serde_json::Value;

use crate::targets::ComputeTarget;

/// What the host reported for one phase of the retag.
pub(in crate::cli::host) struct RetagPhase {
    pub(in crate::cli::host) state: String,
    revision: String,
    tags: String,
}

/// Whether TARGET's owner vault holds `item`: `absent`, `active` or the
/// lifecycle state the vault records. Read by the publisher declaration,
/// which mints an item only when the host does not hold one.
pub(crate) async fn vault_item_state(
    target: &str,
    item: &str,
) -> Result<String, crate::cli::CmdError> {
    let credential_host =
        crate::cli::host::machine::users::credentials::credential_host(target).await?;
    let phase = read_vault_phase(
        &credential_host.target,
        &credential_host.vault,
        item,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(crate::cli::CmdError::click)?;
    Ok(phase.state)
}

/// The id of the one live item in TARGET's owner vault that plays `role`
/// (tagged `stado:role:<role>`), or none: the same listing and the same
/// holder rule `item put` writes by. Two holders are refused, because
/// choosing would be a guess.
pub(crate) async fn vault_role_item(
    target: &str,
    role: &str,
) -> Result<Option<String>, crate::cli::CmdError> {
    let (_, listing) =
        crate::cli::host::secrets::vault::mirror::remote_skarbiec_json(target, &["list".into()])
            .await?;
    let items: Vec<crate::skarbiec::ItemInfo> =
        serde_json::from_value(listing).map_err(|error| {
            crate::cli::CmdError::click(format!(
                "{target}: skarbiec list did not answer item metadata: {error}"
            ))
        })?;
    match crate::skarbiec::roles::holders(&items, role).as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.id.clone())),
        several => Err(crate::cli::CmdError::refused(format!(
            "{target}: {} items carry {}; exactly one item may play role {role}",
            several.len(),
            crate::skarbiec::roles::role_tag(role)
        ))),
    }
}

/// One item of the host's vault, read as a retag phase: its state, revision
/// and tags, or `absent` when the vault holds no such item. The vault is read
/// over the channel and parsed here — the phase rendering the retired
/// script's python snippet produced, without a python payload.
pub(in crate::cli::host) async fn read_vault_phase(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<RetagPhase, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    let Some(record) = document.get("items").and_then(|items| items.get(item)) else {
        return Ok(RetagPhase {
            state: "absent".to_string(),
            revision: "-".to_string(),
            tags: "-".to_string(),
        });
    };
    let state = record
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .unwrap_or("-")
        .to_string();
    let revision = match record.get("revision") {
        Some(Value::String(revision)) => revision.clone(),
        Some(Value::Number(revision)) => revision.to_string(),
        _ => "-".to_string(),
    };
    let tags = record
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<&str>>()
                .join(",")
        })
        .filter(|tags| !tags.is_empty())
        .unwrap_or_else(|| "-".to_string());
    Ok(RetagPhase {
        state,
        revision,
        tags,
    })
}

/// Per-field metadata of one decrypted item, computed on the host.
///
/// The value is what must not travel, and a length and a digest are not the
/// value: they are what lets a workstation prove that the item a declaration
/// references holds the bytes the operator has locally, without either side
/// sending them. So the decryption and the hashing both happen on the host,
/// and only this summary crosses the channel.
#[derive(serde::Deserialize)]
struct VaultFieldSummary {
    name: String,
    length: u64,
    sha256: String,
    text: bool,
}

/// One entry of an item's context: the descriptors Skarbiec keeps beside the
/// secret fields (`login_method`, `account_ref`, `provider`, `product`,
/// `role`). A scalar is reported as it is; an object or array only as
/// structured, because nothing guarantees what a writer nested there.
#[derive(serde::Deserialize)]
struct VaultContextEntry {
    name: String,
    value: Option<Value>,
}

#[derive(serde::Deserialize)]
struct VaultItemSummary {
    kind: Option<String>,
    schema: Option<String>,
    fields: Vec<VaultFieldSummary>,
    /// `None` from a host whose reducer predates context reporting.
    context: Option<Vec<VaultContextEntry>>,
}

/// The reducer the host runs as `stado credentials item summarize-local`:
/// `skarbiec get` writes the decrypted document to a pipe, and this reads it,
/// replaces every field value with its length and SHA-256, and prints the
/// summary with the item's context descriptors ([`VaultContextEntry`]). No
/// field value is printed, so a secret cannot reach the caller even by
/// accident. A text field is hashed as its bytes; any other value as its
/// compact JSON with object keys sorted.
pub fn summarize_local() -> Result<(), crate::cli::CmdError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let refused = |detail: String| crate::cli::CmdError::click(detail);
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| refused(format!("the item document could not be read: {error}")))?;
    let document: Value = serde_json::from_str(&input)
        .map_err(|error| refused(format!("the item document is not JSON: {error}")))?;
    let empty = serde_json::Map::new();
    let fields = document["fields"].as_object().unwrap_or(&empty);
    let mut names: Vec<&String> = fields.keys().collect();
    names.sort();
    let summary: Vec<Value> = names
        .into_iter()
        .map(|name| {
            let value = &fields[name];
            let bytes = match value {
                Value::String(text) => text.clone().into_bytes(),
                other => sorted(other).to_string().into_bytes(),
            };
            serde_json::json!({
                "name": name,
                "text": value.is_string(),
                "length": bytes.len(),
                "sha256": hex::encode(Sha256::digest(&bytes)),
            })
        })
        .collect();
    let context = document["context"].as_object().unwrap_or(&empty);
    let mut keys: Vec<&String> = context.keys().collect();
    keys.sort();
    let context: Vec<Value> = keys
        .into_iter()
        .map(|name| {
            let value = &context[name];
            let reported = (!value.is_object() && !value.is_array()).then(|| value.clone());
            serde_json::json!({ "name": name, "value": reported })
        })
        .collect();
    let report = serde_json::json!({
        "kind": document.get("kind"),
        "schema": document.get("schema"),
        "fields": summary,
        "context": context,
    });
    println!("{report}");
    Ok(())
}

/// VALUE with every object's keys in sorted order.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sorted(&map[key])))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// When the host last wrote this item. Read from the same encrypted record as
/// the revision, so it costs no decryption.
async fn read_vault_updated_at(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    Ok(document
        .get("items")
        .and_then(|items| items.get(item))
        .and_then(|record| record.get("updated_at"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string())
}
