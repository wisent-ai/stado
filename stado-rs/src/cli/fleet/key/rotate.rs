//! Key generation and rotation in the selected credential store.
//!
//! `generate` creates a fresh ed25519 pair for a target. `rotate` installs the
//! new public key through the still-valid old key, overwrites the store item,
//! verifies the channel with the new key, and only then removes the old public
//! key. Failed verification restores the old item.

use serde_json::{json, Value};
use crate::deploy::{CommandSpec, Runner};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    authorized_keys_line, channel_argv, configured_client, item_id, run_checked, settle_readable,
    ITEM_TYPE,
};

struct KeyPair {
    private_key: String,
    public_key: String,
    fingerprint: String,
}
struct GeneratedFiles {
    private: PathBuf,
    public: PathBuf,
}

impl Drop for GeneratedFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.private);
        let _ = std::fs::remove_file(&self.public);
    }
}

/// Generate an ed25519 pair in a unique transient path guarded by Drop.
async fn generate_pair(runner: &Runner, comment: &str) -> Result<KeyPair, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("stado-fleet-keygen-{}-{nonce}", std::process::id()));
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
    let files = GeneratedFiles {
        private: path,
        public: PathBuf::from(format!("{path_str}.pub")),
    };
    let private_key = std::fs::read_to_string(&files.private).map_err(|exc| exc.to_string())?;
    let public_key = std::fs::read_to_string(&files.public).map_err(|exc| exc.to_string())?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec!["ssh-keygen".to_string(), "-lf".to_string(), path_str]),
        "ssh-keygen -lf",
    )
    .await?;
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

/// Store the pair, grant the channel's reader, and read the public half back
/// through that same reader.
///
/// A stored key is not a usable key. Skarbiec authorizes reads per item, so the
/// item this just wrote is readable by nobody until the channel's consumer is
/// granted its fields — a mint that stopped at the write handed the operator a
/// public key to install and a host that could never be reached. Granting is
/// therefore part of minting, and the read-back through the channel's own
/// consumer is what says so.
async fn store_pair(
    client: &crate::skarbiec::Client,
    target: &str,
    pair: &KeyPair,
) -> Result<(), String> {
    let id = item_id(target);
    client
        .write_described(
            &id,
            ITEM_TYPE,
            &json!({
                "private_key": pair.private_key,
                "public_key": pair.public_key,
            }),
            &json!({
                "key_type": "ED25519",
                "fingerprint": pair.fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .map_err(|exc| exc.to_string())?;
    // Only the public half is verified here: it is the value the caller is about
    // to install on the new machine, it travels through the same grant the
    // private half does, and reading a private key to compare it earns nothing.
    settle_readable(client, &id, &[("public_key", pair.public_key.trim())]).await
}

/// `key generate TARGET` — store a fresh pair and print only the public key.
pub async fn generate(runner: &Runner, target: &str) -> Result<bool, String> {
    let pair = generate_pair(runner, &item_id(target)).await?;
    let client = configured_client()?;
    store_pair(&client, target, &pair).await?;
    println!(
        "stored credential item {} ({})",
        item_id(target),
        pair.fingerprint
    );
    println!("public key: {}", pair.public_key);
    Ok(true)
}

/// `key rotate TARGET` — replace the target key end to end, restoring the old
/// credential-store item if the new key cannot open the channel.
pub async fn rotate(runner: &Runner, target: &str) -> Result<bool, String> {
    let client = configured_client()?;
    // The rollback below writes this item back, so both halves of the pair and
    // the description beside them are read explicitly. `private_key` and
    // `public_key` are the pair's fields; the fingerprint and key type are
    // context. Losing either half here would make the rollback restore an
    // unusable credential.
    let mut old_fields = serde_json::Map::new();
    for field in ["private_key", "public_key"] {
        if let Some(value) = client
            .read_string(&item_id(target), field)
            .await
            .map_err(|exc| exc.to_string())?
        {
            old_fields.insert(field.to_string(), Value::from(value));
        }
    }
    let old_context = client
        .read_field(&item_id(target), "context")
        .await
        .unwrap_or_else(|_| json!({}));
    let old_fields = Value::Object(old_fields);
    let old_fingerprint = old_context
        .get("fingerprint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let old_public = old_fields
        .get("public_key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let pair = generate_pair(runner, &item_id(target)).await?;
    // The channel still resolves the old stored key while installing the new
    // public key.
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
                .write_described(&item_id(target), ITEM_TYPE, &old_fields, &old_context)
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
    let (argv, _key) = channel_argv(target, &destination, &command).await?;
    run_checked(runner, CommandSpec::new(argv), "authorized_keys install")
        .await
        .map(|_| ())
}

async fn remove_public_key(runner: &Runner, target: &str, public_key: &str) -> Result<(), String> {
    if public_key.is_empty() {
        return Ok(());
    }
    let destination = destination_of(target).await?;
    let command = format!(
        "grep -vF '{public_key}' \"$HOME/.ssh/authorized_keys\" > \"$HOME/.ssh/authorized_keys.tmp\" && mv \"$HOME/.ssh/authorized_keys.tmp\" \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, &destination, &command).await?;
    run_checked(runner, CommandSpec::new(argv), "authorized_keys cleanup")
        .await
        .map(|_| ())
}

async fn verify_new_key(runner: &Runner, target: &str) -> Result<String, String> {
    let destination = destination_of(target).await?;
    let (argv, _key) = channel_argv(target, &destination, "hostname").await?;
    Ok(
        run_checked(runner, CommandSpec::new(argv), "hostname with the new key")
            .await?
            .trim()
            .to_string(),
    )
}

async fn destination_of(target: &str) -> Result<String, String> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?
        .ssh
        .clone()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))
}
