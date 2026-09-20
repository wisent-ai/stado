//! `stado registry host show <HOST> [--path FIELD]` — one host's declaration,
//! answered by the command instead of grepped out of a dump.
//!
//! The registry is one document and a question about one host is the usual
//! question. Until now the answer was `stado registry pull` into a file and a
//! `jq` over it, or `pull --path targets.<host>` for a reader who already
//! knows how the document is shaped. Oko's one-off catalogue holds eleven such
//! pulls into `~/.oko/registry-pull.json`, and every one of them is a copy of
//! operator state that goes stale the moment it lands.
//!
//! The host is found the way the rest of Stado finds one — the `targets` array,
//! by `name` — and an unknown name is refused with the names there, so the next
//! attempt is not a guess.

use serde_json::Value;

use crate::cli::registry::commands::pull::select;
use crate::cli::CmdError;
use crate::targets::RegistryStore;

/// Where the registry keeps its hosts.
const TARGETS: &str = "targets";
const NAME_FIELD: &str = "name";

pub async fn host_show(host: &str, path: Option<&str>) -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    let document: Value = serde_json::from_str(&blob.content)?;
    let targets = document
        .get(TARGETS)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CmdError::click(format!(
                "registry at {} carries no `{TARGETS}` array",
                store.location()
            ))
        })?;
    let found = targets
        .iter()
        .find(|target| target.get(NAME_FIELD).and_then(Value::as_str) == Some(host))
        .ok_or_else(|| {
            let mut names: Vec<&str> = targets
                .iter()
                .filter_map(|target| target.get(NAME_FIELD).and_then(Value::as_str))
                .collect();
            names.sort_unstable();
            CmdError::click(format!(
                "registry has no host `{host}`; hosts there: {}",
                names.join(", ")
            ))
        })?;
    let part = match path {
        Some(path) => select(found, path)?,
        None => found,
    };
    match part {
        Value::String(text) => println!("{text}"),
        other => println!("{}", serde_json::to_string_pretty(other)?),
    }
    Ok(())
}
