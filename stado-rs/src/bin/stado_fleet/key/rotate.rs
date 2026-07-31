//! Key generation and rotation for vault host keys.
//!
//! `generate` creates a fresh ed25519 pair for a target and stores it in
//! the vault; the public key is printed for installation. `rotate` is the
//! safe version of the same act on a live host: install the new public
//! key through the still-valid old key, overwrite the vault item, verify
//! the channel with the NEW key, and only then remove the old public key.
//! A failed verification restores the old vault item, so a rotation never
//! strands the host on a key nobody holds.

use serde_json::{json, Value};
use stado::deploy::{CommandSpec, Runner};

use super::{authorized_keys_line, channel_argv, configured_client, item_id, run_checked, ITEM_TYPE};

struct KeyPair {
    private_key: String,
    public_key: String,
    fingerprint: String,
}

/// Generate an ed25519 pair in the transient directory; both files are
/// removed by the caller after the material is read.
async fn generate_pair(runner: &Runner, comment: &str) -> Result<KeyPair, String> {
    let path = std::env::temp_dir().join(format!("stado-fleet-keygen-{}", std::process::id()));
    let path_str = path.to_string_lossy().to_string();
    run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-t".to_string(),
            "ed25519".to_string(),
            "-f".to_string(),
            path_str.clone(),
            "-N".to_string(),
            String::new(),
            "-C".to_string(),
            comment.to_string(),
        ]),
        "ssh-keygen ed25519",
    )
    .await?;
    let private_key = std::fs::read_to_string(&path).map_err(|exc| exc.to_string())?;
    let public_key =
        std::fs::read_to_string(format!("{path_str}.pub")).map_err(|exc| exc.to_string())?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-lf".to_string(),
            path_str.clone(),
        ]),
        "ssh-keygen -lf",
    )
    .await?;
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path_str}.pub"));
    let fingerprint = fingerprint_line
        .split_whitespace()
        .find(|part| part.starts_with("SHA256:"))
        .unwrap_or_default()
        .to_string();
    Ok(KeyPair {
        private_key: private_key.trim().to_string(),
        public_key: public_key.trim().to_string(),
        fingerprint,
    })
}

async fn store_pair(client: &stado::skarbiec::Client, target: &str, pair: &KeyPair) -> Result<(), String> {
    client
        .write_item(
            &item_id(target),
            ITEM_TYPE,
            &json!({
                "private_key": pair.private_key,
                "public_key": pair.public_key,
                "key_type": "ED25519",
                "fingerprint": pair.fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .map_err(|exc| exc.to_string())
}

/// `key generate TARGET` — fresh pair into the vault; the public key is
/// printed so it can be installed wherever the host accepts keys.
pub async fn generate(runner: &Runner, target: &str) -> Result<bool, String> {
    let pair = generate_pair(runner, &item_id(target)).await?;
    let client = configured_client().await?;
    store_pair(&client, target, &pair).await?;
    println!("stored vault item {} ({})", item_id(target), pair.fingerprint);
    println!("public key: {}", pair.public_key);
    Ok(true)
}

/// `key rotate TARGET` — replace the target's key end to end, rolling
/// back the vault item when the new key cannot open the channel.
pub async fn rotate(runner: &Runner, target: &str) -> Result<bool, String> {
    let client = configured_client().await?;
    let old_item = client
        .read_item(&item_id(target))
        .await
        .map_err(|exc| exc.to_string())?;
    let old_fingerprint = old_item
        .get("fingerprint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let old_public = old_item
        .get("public_key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let pair = generate_pair(runner, &item_id(target)).await?;
    // The channel still resolves the OLD vault key at this point: install
    // the new public key through it.
    install_public_key(runner, target, &pair.public_key).await?;
    store_pair(&client, target, &pair).await?;
    match verify_new_key(runner, target).await {
        Ok(answered) => {
            remove_public_key(runner, target, &old_public).await?;
            println!(
                "rotated '{target}': {old_fingerprint} -> {} (answered as {answered})",
                pair.fingerprint
            );
            Ok(true)
        }
        Err(exc) => {
            client
                .write_item(&item_id(target), ITEM_TYPE, &old_item)
                .await
                .map_err(|err| err.to_string())?;
            let _ = remove_public_key(runner, target, &pair.public_key).await;
            Err(format!(
                "new key could not open the channel ({exc}); the old key was restored"
            ))
        }
    }
}

async fn install_public_key(runner: &Runner, target: &str, public_key: &str) -> Result<(), String> {
    let destination = destination_of(target).await?;
    let line = authorized_keys_line(public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, materialized) = channel_argv(runner, target, &destination, &command).await?;
    let result = run_checked(runner, CommandSpec::new(argv), "authorized_keys install").await;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    result.map(|_| ())
}

async fn remove_public_key(runner: &Runner, target: &str, public_key: &str) -> Result<(), String> {
    if public_key.is_empty() {
        return Ok(());
    }
    let destination = destination_of(target).await?;
    let command = format!(
        "grep -vF '{public_key}' \"$HOME/.ssh/authorized_keys\" > \"$HOME/.ssh/authorized_keys.tmp\" && mv \"$HOME/.ssh/authorized_keys.tmp\" \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, materialized) = channel_argv(runner, target, &destination, &command).await?;
    let result = run_checked(runner, CommandSpec::new(argv), "authorized_keys cleanup").await;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    result.map(|_| ())
}

async fn verify_new_key(runner: &Runner, target: &str) -> Result<String, String> {
    let destination = destination_of(target).await?;
    let (argv, materialized) = channel_argv(runner, target, &destination, "hostname").await?;
    let result = run_checked(runner, CommandSpec::new(argv), "hostname with the new key").await;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    Ok(result?.trim().to_string())
}

async fn destination_of(target: &str) -> Result<String, String> {
    let registry = stado::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?
        .ssh
        .clone()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))
}
