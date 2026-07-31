//! Selected-store SSH channel materialization.

#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use stado::skarbiec::{Client, SkarbiecError};

use super::{configured_client, item_id};

/// Owner-only transient private key. Drop removes the file on success, error,
/// and early return, so callers cannot forget cleanup.
pub struct KeyFile(PathBuf);

impl KeyFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for KeyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn missing_key(id: &str, error: SkarbiecError) -> String {
    match error {
        SkarbiecError::MissingValue(_) => format!(
            "credential store has no SSH key item {id:?}; run `stado_fleet key add` or `key generate`"
        ),
        SkarbiecError::Response { status, .. }
            if status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
        {
            format!(
                "credential store has no SSH key item {id:?}; run `stado_fleet key add` or `key generate`"
            )
        }
        other => other.to_string(),
    }
}

#[cfg(unix)]
fn write_key(private_key: &str) -> Result<KeyFile, String> {
    use std::os::unix::fs::OpenOptionsExt;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "stado-fleet-key-{}-{nonce}",
        std::process::id()
    ));
    let owner_mode = u32::from_str_radix("600", u32::from(u8::BITS))
        .map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(owner_mode)
        .open(&path)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file
        .write_all(format!("{private_key}\n").as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = std::fs::remove_file(&path);
        return Err(error.to_string());
    }
    Ok(KeyFile(path))
}

#[cfg(not(unix))]
fn write_key(private_key: &str) -> Result<KeyFile, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "stado-fleet-key-{}-{nonce}",
        std::process::id()
    ));
    std::fs::write(&path, format!("{private_key}\n")).map_err(|error| error.to_string())?;
    Ok(KeyFile(path))
}

async fn materialize(client: &Client, target: &str) -> Result<KeyFile, String> {
    let id = item_id(target);
    let item = client
        .read_item(&id)
        .await
        .map_err(|error| missing_key(&id, error))?;
    let private_key = item
        .get("private_key")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("credential item {id} has no private_key field"))?;
    write_key(private_key)
}

/// Build one SSH invocation using only the target key in the selected store.
pub async fn channel_argv(
    target: &str,
    destination: &str,
    command: &str,
) -> Result<(Vec<String>, KeyFile), String> {
    let client = configured_client()?;
    let key = materialize(&client, target).await?;
    let argv = vec![
        "ssh".to_string(),
        "-i".to_string(),
        key.path().to_string_lossy().to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        destination.to_string(),
        command.to_string(),
    ];
    Ok((argv, key))
}
