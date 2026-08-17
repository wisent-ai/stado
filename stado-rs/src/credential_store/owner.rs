//! Owner-path writes into a Skarbiec vault.
//!
//! Skarbiec's `PUT /v1/items` is not a general item write and has not been one
//! since the vault contracts were rebuilt (Skarbiec 9aa7dd4, 2026-08-04). The
//! route now requires `id`, `field` and `operation_id`, and outside
//! `mode=acquire` it refuses anything that is not controlled by the exact Weles
//! writer presenting the grant. Stado's client still sent the whole item, so the
//! broker answered every write — `stado credentials put`, `stado_fleet key
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
/// Vault the fleet's operator items live in when nothing overrides it.
const VAULT_CANDIDATE: &str = "$HOME/.stado/skarbiec.vault.json";

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
    let path = PathBuf::from(VAULT_CANDIDATE.replace("$HOME", &home()?));
    if path.is_file() {
        return Ok(path);
    }
    Err(SkarbiecError::Deployment(format!(
        "no owner vault at {}; this machine cannot write credential items. \
         Set SKARBIEC_VAULT_FILE, or run the write on the host that holds the vault \
         (`stado host vaults` names them)",
        path.display()
    )))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_override_must_name_a_file() {
        let _guard = super::super::test_env_lock();
        std::env::set_var("SKARBIEC_BIN", "/nonexistent/skarbiec");
        let error = binary().expect_err("a missing override is an error");
        assert!(error.to_string().contains("SKARBIEC_BIN names no file"));
        std::env::remove_var("SKARBIEC_BIN");
    }

    #[test]
    fn vault_override_must_name_a_file() {
        let _guard = super::super::test_env_lock();
        std::env::set_var("SKARBIEC_VAULT_FILE", "/nonexistent/vault.json");
        let error = vault().expect_err("a missing override is an error");
        assert!(error
            .to_string()
            .contains("SKARBIEC_VAULT_FILE names no file"));
        std::env::remove_var("SKARBIEC_VAULT_FILE");
    }
}
