//! `stado registry host add` — onboard a machine into the canonical
//! registry, validated before anything is written.

use serde_json::{json, Value};

use crate::cli::registry::write::document::{fetch_versioned_document, push_document_if};
use crate::cli::CmdError;
use crate::targets;

/// `stado registry host add HOST --ssh DEST [--kind local]` — onboard a
/// machine into the canonical registry.
///
/// Refuses a name the registry already declares, and runs the exact
/// validation [`push`](crate::cli::registry::push) runs BEFORE anything is written, so a colliding
/// hostname alias or an ssh destination with no host is rejected with the
/// registry-v2 contract's own message instead of landing in the store.
pub async fn host_add(
    host: &str,
    ssh: &str,
    kind: &str,
    release_platform: &str,
) -> Result<(), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    if ssh.trim().is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    let location = targets::registry_location();
    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let entries = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let duplicate = entries.iter().find(|entry| {
        entry
            .get("name")
            .and_then(Value::as_str)
            .map(targets::normalize_hostname)
            .as_deref()
            == Some(name.as_str())
    });
    if let Some(entry) = duplicate {
        let declared_kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(CmdError::click(format!(
            "{name} is already declared in {location} (kind={declared_kind}); \
             refusing to add a duplicate"
        )));
    }
    entries.push(json!({
        "name": name,
        "kind": kind,
        "ssh": ssh,
        "release_platform": release_platform,
        "notes": "onboarded by `stado registry host add`",
    }));
    let generation = push_document_if(&document, &expected_generation).await?;
    println!(
        "added {name} (kind={kind}, release_platform={release_platform}, ssh={ssh}) -> \
         {location} generation={generation}"
    );
    Ok(())
}
