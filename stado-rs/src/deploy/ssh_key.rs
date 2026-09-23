//! Credential-store SSH identity materialization for every managed host channel.

#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::DeployError;
use crate::skarbiec::{Client, SkarbiecError};

const ITEM_PREFIX: &str = "stado-ssh-";

/// Owner-only transient private key. Dropping it removes the file on every
/// success and error path.
pub struct KeyFile(PathBuf);

impl KeyFile {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for KeyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn item_id(target: &str) -> String {
    format!("{ITEM_PREFIX}{target}")
}

fn missing_key(id: &str, error: SkarbiecError) -> DeployError {
    match error {
        SkarbiecError::MissingValue(_) => DeployError(format!(
            "credential store has no SSH key item {id:?}; run `stado_fleet key add` or `key generate`"
        )),
        SkarbiecError::Response { status, .. }
            if status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
        {
            DeployError(format!(
                "credential store has no SSH key item {id:?}; run `stado_fleet key add` or `key generate`"
            ))
        }
        other => DeployError(other.to_string()),
    }
}

#[cfg(unix)]
fn write_key(private_key: &str) -> Result<KeyFile, DeployError> {
    use std::os::unix::fs::OpenOptionsExt;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DeployError(error.to_string()))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("stado-host-key-{}-{nonce}", std::process::id()));
    let owner_mode = u32::from_str_radix("600", u32::from(u8::BITS))
        .map_err(|error| DeployError(error.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(owner_mode)
        .open(&path)
        .map_err(|error| DeployError(error.to_string()))?;
    if let Err(error) = file
        .write_all(format!("{private_key}\n").as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = std::fs::remove_file(&path);
        return Err(DeployError(error.to_string()));
    }
    Ok(KeyFile(path))
}

#[cfg(not(unix))]
fn write_key(private_key: &str) -> Result<KeyFile, DeployError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DeployError(error.to_string()))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("stado-host-key-{}-{nonce}", std::process::id()));
    std::fs::write(&path, format!("{private_key}\n"))
        .map_err(|error| DeployError(error.to_string()))?;
    Ok(KeyFile(path))
}

/// Materialize the target-scoped private key through the operator bootstrap
/// grant. Private material never enters argv, stdout, logs, or registry data.
pub async fn materialize(target: &str) -> Result<KeyFile, DeployError> {
    let id = item_id(target);
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| DeployError(error.to_string()))?;
    let client = Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|error| DeployError(error.to_string()))?;
    let item = client
        .read_item(&id)
        .await
        .map_err(|error| missing_key(&id, error))?;
    let private_key = item
        .get("private_key")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError(format!("credential item {id} has no private_key field")))?;
    write_key(private_key)
}

/// Force OpenSSH to use only the target-scoped key. The first argv word must be
/// `ssh` or `scp`; callers retain the returned [`KeyFile`] until the process
/// exits.
pub fn add_identity(mut argv: Vec<String>, key: &KeyFile) -> Result<Vec<String>, DeployError> {
    if !matches!(argv.first().map(String::as_str), Some("ssh" | "scp")) {
        return Err(DeployError(
            "SSH identity can only be attached to an ssh or scp invocation".to_string(),
        ));
    }
    let after_program = usize::from(true);
    argv.splice(
        after_program..after_program,
        [
            "-i".to_string(),
            key.path().to_string_lossy().to_string(),
            "-o".to_string(),
            "IdentitiesOnly=yes".to_string(),
        ],
    );
    Ok(argv)
}
