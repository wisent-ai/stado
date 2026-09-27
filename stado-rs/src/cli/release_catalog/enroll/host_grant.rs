//! Adding a product's build secrets to one host's workload secret declaration.
//!
//! Each host keeps its own lists: the vault owner allows items this machine
//! does not, and the other way round. Writing this machine's lists onto the
//! owner replaced the owner's with ours, and the owner's validator refused the
//! result (`weles-admission-api#token names an item absent from
//! agent.skarbiec.items`). So each host's own lists are read, the missing
//! references are added to them, and `items` is written before `secret_fields`:
//! the validator judges every field against the items already on disk.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{remote_config_output, write_host_config, RemoteConfigAction};
use crate::cli::CmdError;

const ITEMS_KEY: &str = "agent.skarbiec.items";
const FIELDS_KEY: &str = "agent.skarbiec.secret_fields";
/// Where `stado config show` reports the two lists it resolved.
const RESOLVED_ITEMS: &str = "/resolved/agent_skarbiec_items";
const RESOLVED_FIELDS: &str = "/resolved/agent_skarbiec_secret_fields";

fn strings(document: &Value, pointer: &str) -> BTreeSet<String> {
    document
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Add every `(item, field)` in `missing` to `host`'s own declaration.
pub(super) async fn declare_on_host(
    host: &str,
    missing: &[&(String, String)],
) -> Result<(), CmdError> {
    let target = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let shown = remote_config_output(
        &target,
        RemoteConfigAction::Show,
        &crate::deploy::production_runner(),
    )
    .await?;
    let document: Value = serde_json::from_str(&shown).map_err(|error| {
        CmdError::click(format!(
            "{host}: stado config show did not answer JSON: {error}"
        ))
    })?;
    let mut items = strings(&document, RESOLVED_ITEMS);
    let mut fields = strings(&document, RESOLVED_FIELDS);
    let (before_items, before_fields) = (items.len(), fields.len());
    for (item, field) in missing {
        items.insert(item.clone());
        fields.insert(format!("{item}#{field}"));
    }
    // A refusal names the host and the lists it judged: without them one
    // "config unchanged" from two hosts could not be told apart.
    let context = |key: &str, error: CmdError| {
        CmdError::click(format!(
            "{host}: writing {key} failed ({before_items} items and {before_fields} fields \
             read from its stado config show, adding {}): {error}",
            missing
                .iter()
                .map(|(item, field)| format!("{item}#{field}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    if items.len() != before_items {
        write_host_config(host, ITEMS_KEY, &json!(items).to_string())
            .await
            .map_err(|error| context(ITEMS_KEY, error))?;
    }
    if fields.len() != before_fields {
        write_host_config(host, FIELDS_KEY, &json!(fields).to_string())
            .await
            .map_err(|error| context(FIELDS_KEY, error))?;
    }
    Ok(())
}
