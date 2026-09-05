//! Owner-path writes into a Skarbiec vault.
//!
//! Skarbiec's `PUT /v1/items` is not a general item write and has not been one
//! since the vault contracts were rebuilt (Skarbiec 9aa7dd4, 2026-08-04). The
//! route now requires `id`, `field` and `operation_id`, and outside
//! `mode=acquire` it refuses anything that is not controlled by the exact Weles
//! writer presenting the grant. Stado's client still sent the whole item, so the
//! broker answered every write — `stado credentials put`, `stado fleet key
//! generate`, `key add`, `key rotate`, the Azure operator credential — with a
//! bare `400 {"error":"field required"}`. The fleet could read its credentials
//! and could not mint one, which is why a new host could not be enrolled at all.
//!
//! An item the operator owns is written the way its owner writes it: through the
//! `skarbiec` CLI against the vault file, which holds the owner key. That is the
//! same call `stado credentials harvest --restore` already made for a Skarbiec
//! selector; it lives here now so every write in the process shares it instead
//! of one path knowing the contract and the rest guessing.
//!
//! Field placement belongs to Skarbiec's schema, not to callers: a `ssh-key`
//! payload normalizes to kind `key-pair` with `private_key`/`public_key` as
//! fields and `fingerprint`/`key_type` as context. Sending the flat object is
//! correct; assuming where each key lands on the way out is not.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::skarbiec::SkarbiecError;

/// Where Stado installs Skarbiec, mirroring
/// [`crate::deploy::host_recovery::WC_CANDIDATES`]: one prefix, discovered the
/// same way, so the two cannot drift apart.
const SKARBIEC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec"];
/// Envelope every owner write carries.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";
/// Vaults the fleet's operator items may live in when nothing overrides it,
/// in the order `skarbiec`'s own `vaults` command searches them, so Stado and
/// Skarbiec cannot answer "which vault" differently.
const VAULT_CANDIDATES: &[&str] = &[
    "$HOME/.local/share/skarbiec/skarbiec.vault.json",
    "$HOME/.stado/skarbiec.vault.json",
    "$HOME/skarbiec.vault.json",
];

fn home() -> Result<String, SkarbiecError> {
    std::env::var("HOME").map_err(|_| SkarbiecError::Deployment("HOME is not set".to_string()))
}

/// Resolve the installed `skarbiec` binary.
///
/// `SKARBIEC_BIN` is the override the credential scripts already use, and it is
/// the only way to exercise a build before it is installed — which is the
/// situation whenever the installed binary is the thing that is stale.
pub fn binary() -> Result<PathBuf, SkarbiecError> {
    if let Ok(explicit) = std::env::var("SKARBIEC_BIN") {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(SkarbiecError::Deployment(format!(
                "SKARBIEC_BIN names no file: {explicit}"
            )));
        }
        return Ok(path);
    }
    let home = home()?;
    for candidate in SKARBIEC_CANDIDATES {
        let path = PathBuf::from(candidate.replace("$HOME", &home));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(SkarbiecError::Deployment(format!(
        "no installed skarbiec binary at {}",
        SKARBIEC_CANDIDATES.join(", ")
    )))
}

/// Resolve the owner vault this process writes through.
///
/// A machine that does not hold the vault cannot own a credential write, and
/// saying so is the whole point: the alternative is a write that appears to
/// succeed against a store no owner here can open.
///
/// The discovery order is Skarbiec's own — `$HOME/.local/share/skarbiec`,
/// then `$HOME/.stado`, then `$HOME`, the list `skarbiec`'s `vaults` command
/// searches. Stado used to name `$HOME/.stado/skarbiec.vault.json` alone,
/// while the `skarbiec` CLI defaults to `.local/share/skarbiec`. Two tools on
/// one machine, two answers, and no way for an operator to see the
/// disagreement: on 2026-09-05 six `skarbiec set-json` writes went to
/// `.local/share/skarbiec` and were simultaneously real, `active` on the
/// host, and invisible to `stado host reconcile-release-verifier`, which read
/// the other file. That closed the fleet's release publication boundary for
/// every product until the declarations were retracted.
///
/// When two candidates carry the SAME owner identity the machine has no
/// single authoritative vault, and picking either silently is exactly the
/// failure above. That is refused, naming both paths and their item counts,
/// because an operator who is told can set `SKARBIEC_VAULT_FILE` and a
/// program that guesses cannot be corrected. The contents are never merged
/// here: which items belong where is the operator's decision.
pub fn vault() -> Result<PathBuf, SkarbiecError> {
    if let Ok(explicit) = std::env::var("SKARBIEC_VAULT_FILE") {
        if !explicit.trim().is_empty() {
            let path = PathBuf::from(explicit.trim());
            if !path.is_file() {
                return Err(SkarbiecError::Deployment(format!(
                    "SKARBIEC_VAULT_FILE names no file: {}",
                    path.display()
                )));
            }
            return Ok(path);
        }
    }
    let home = home()?;
    let candidates: Vec<PathBuf> = VAULT_CANDIDATES
        .iter()
        .map(|candidate| PathBuf::from(candidate.replace("$HOME", home.as_str())))
        .collect::<Vec<_>>();
    let present: Vec<(PathBuf, String, usize)> = candidates
        .iter()
        .filter(|path| path.is_file())
        .filter_map(|path| vault_identity(path).map(|(owner, items)| (path.clone(), owner, items)))
        .collect();
    if present.len() > usize::from(true) {
        let first_owner = &present[usize::default()].1;
        if present.iter().all(|(_, owner, _)| owner == first_owner) {
            let described = present
                .iter()
                .map(|(path, _, items)| format!("{} ({items} items)", path.display()))
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(SkarbiecError::Deployment(format!(
                "this machine holds {} vaults that all claim owner {first_owner}: {described}. \
                 There is no single authoritative vault, so a credential write or an \
                 authoritative read here would silently pick one — which is how six real items \
                 became invisible to the release verifier. Name the one you mean with \
                 SKARBIEC_VAULT_FILE, or reconcile them; nothing is merged for you.",
                present.len()
            )));
        }
    }
    if let Some((path, _, _)) = present.into_iter().next() {
        return Ok(path);
    }
    Err(SkarbiecError::Deployment(format!(
        "no owner vault in {}; this machine cannot write credential items. \
         Set SKARBIEC_VAULT_FILE, or run the write on the host that holds the vault \
         (`stado host vaults` names them)",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// One candidate's owner identity and item count, or `None` when it is not a
/// vault at all — a backup or a half-written file is simply not a candidate.
fn vault_identity(path: &std::path::Path) -> Option<(String, usize)> {
    let document: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?)
        .ok()
        .filter(serde_json::Value::is_object)?;
    let owner = document
        .get("owner")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            document
                .get("management")
                .and_then(|management| management.get("controller"))
                .and_then(serde_json::Value::as_str)
        })?
        .to_string();
    let items = document
        .get("items")
        .and_then(serde_json::Value::as_object)
        .map(serde_json::Map::len)
        .unwrap_or_default();
    Some((owner, items))
}

/// Write one item into an explicit vault through its owner.
///
/// `set-json` takes a canonical payload and validates it: the kind must be one
/// Skarbiec declares, every key in `fields` must be one that kind allows, and
/// anything descriptive belongs in `context`. So `ssh-key` is not a kind — it is
/// a `key-pair` whose fingerprint and key type are context — and passing the
/// wrong one is refused rather than stored in a shape no reader expects.
///
/// `SKARBIEC_UNLOCK`/`SKARBIEC_UNLOCK_FILE` are removed for the child: an unlock
/// phrase inherited from this process's environment would decide which vault key
/// is used without any caller having asked for it. The payload travels on stdin,
/// never in argv, because argv is readable by every process on the machine.
pub fn store_json(
    binary: &Path,
    vault: &Path,
    item: &str,
    item_type: &str,
    fields: &Value,
    context: &Value,
) -> Result<(), SkarbiecError> {
    let mut child = std::process::Command::new(binary)
        .arg("set-json")
        .arg(item)
        .arg("--type")
        .arg(item_type)
        .env("SKARBIEC_VAULT_FILE", vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    let payload = json!({
        "schema": ITEM_SCHEMA,
        "kind": item_type,
        "fields": fields,
        "context": context,
    });
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(payload.to_string().as_bytes())
            .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not store {item}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Check the owner vault itself for one live item.
///
/// Reads and writes must use the same vault. Consulting the broker list here
/// can report an item absent while the owner vault already holds its signing
/// key, which would rotate that key during an otherwise idempotent bootstrap.
pub fn item_exists(id: &str) -> Result<bool, SkarbiecError> {
    let output = std::process::Command::new(binary()?)
        .arg("list")
        .env("SKARBIEC_VAULT_FILE", vault()?)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not list owner vault: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let items: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|error| {
        SkarbiecError::Deployment(format!("skarbiec owner list is not valid JSON: {error}"))
    })?;
    Ok(items.iter().any(|item| {
        item.get("id").and_then(Value::as_str) == Some(id)
            && item.get("deleted").and_then(Value::as_bool) != Some(true)
    }))
}

/// Read one exact string field from the resolved owner vault.
///
/// The value is captured from stdin/stdout only and never enters argv. This is
/// the owner-side counterpart to broker reads for bootstrap credentials whose
/// workload grants deliberately exclude the Stado control process.
pub fn read_string(id: &str, field: &str) -> Result<String, SkarbiecError> {
    let output = std::process::Command::new(binary()?)
        .arg("get")
        .arg(id)
        .env("SKARBIEC_VAULT_FILE", vault()?)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not read {id}.{field}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| SkarbiecError::Deployment(format!("{id} is not valid JSON: {error}")))?;
    let value = document
        .get("fields")
        .and_then(Value::as_object)
        .and_then(|fields| fields.get(field))
        .and_then(Value::as_str)
        .ok_or_else(|| SkarbiecError::Deployment(format!("{id} has no string field {field}")))?
        .to_string();
    if value.is_empty() {
        return Err(SkarbiecError::Deployment(format!("{id}.{field} is empty")));
    }
    Ok(value)
}

/// Write one item into the resolved owner vault.
pub fn write_item(
    id: &str,
    item_type: &str,
    fields: &Value,
    context: &Value,
) -> Result<(), SkarbiecError> {
    store_json(&binary()?, &vault()?, id, item_type, fields, context)
}

/// Delete one item from the resolved owner vault.
pub fn delete_item(id: &str) -> Result<(), SkarbiecError> {
    let binary = binary()?;
    let vault = vault()?;
    let output = std::process::Command::new(&binary)
        .arg("delete")
        .arg(id)
        .env("SKARBIEC_VAULT_FILE", &vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not delete {id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
