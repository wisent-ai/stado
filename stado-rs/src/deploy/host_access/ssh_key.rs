//! Credential-store SSH identity materialization for every managed host channel.

#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::deploy::DeployError;
use crate::skarbiec::{Client, SkarbiecError};

const ITEM_PREFIX: &str = "stado-ssh-";

/// The one field these items carry, and the only one this ever needs.
const PRIVATE_KEY_FIELD: &str = "private_key";
const OWNER_KEY_FILE_ENV: &str = "STADO_HOST_SSH_KEY_FILE";

/// Owner-only transient private key. The final handle removes the file on
/// success, error, and cancellation.
#[derive(Clone)]
pub struct KeyFile(Arc<OwnedKeyFile>);

struct OwnedKeyFile {
    path: PathBuf,
}

impl KeyFile {
    pub fn path(&self) -> &Path {
        &self.0.path
    }
}

impl Drop for OwnedKeyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn item_id(target: &str) -> String {
    format!("{ITEM_PREFIX}{target}")
}

fn missing_key(id: &str, error: SkarbiecError) -> DeployError {
    // `is_missing` sees through a named read: the vault answering "not there"
    // is the same fact whether or not the failure carries the consumer, item
    // and field it came from.
    if error.is_missing() {
        return DeployError(format!(
            "credential store has no SSH key item {id:?}; run `stado fleet key add` or `key generate`"
        ));
    }
    DeployError(error.to_string())
}

/// The refusal when the store itself could not be reached.
///
/// On 2026-09-21 repairing charless-mac-mini needed
/// `stado-ssh-charless-mac-mini`, and the read went to
/// `http://127.0.0.1:17602/v1/items/read` — a Stado forward to the Skarbiec
/// ON THAT HOST, which was the thing being repaired. The command said only
/// "error sending request for url …", so the circle was invisible and the
/// recovery channel that already exists went unmentioned.
fn unreachable_store(
    id: &str,
    target: &str,
    url: &str,
    consumer: &str,
    error: SkarbiecError,
) -> DeployError {
    DeployError(format!(
        "reading {id} as {consumer} from the credential store at {url} failed: {error}. That \
         store may be the one {target} serves, in which case repairing {target} needs a key only \
         {target} can hand out. The owner-only way out of that circle is \
         {OWNER_KEY_FILE_ENV}=<path to this host's key>, which this command reads before it asks \
         any broker; `stado fleet key add {target}` puts the same key in a vault that answers."
    ))
}

#[cfg(unix)]
fn write_key(private_key: &str) -> Result<KeyFile, DeployError> {
    use std::os::unix::fs::OpenOptionsExt;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DeployError(error.to_string()))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("stado-host-key-{}-{nonce}", std::process::id()));
    let owner_mode =
        u32::from_str_radix("600", u8::BITS).map_err(|error| DeployError(error.to_string()))?;
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
    Ok(KeyFile(Arc::new(OwnedKeyFile { path })))
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
    Ok(KeyFile(Arc::new(OwnedKeyFile { path })))
}
fn owner_key_override() -> Result<Option<KeyFile>, DeployError> {
    let Some(raw) = std::env::var_os(OWNER_KEY_FILE_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(DeployError(format!(
            "{OWNER_KEY_FILE_ENV} must name an absolute owner-only regular file"
        )));
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        DeployError(format!(
            "cannot inspect {OWNER_KEY_FILE_ENV} {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(DeployError(format!(
            "{OWNER_KEY_FILE_ENV} must name an owner-only regular file"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(DeployError(format!(
                "{OWNER_KEY_FILE_ENV} must not grant group or other permissions"
            )));
        }
    }
    let private_key = std::fs::read_to_string(&path).map_err(|error| {
        DeployError(format!(
            "cannot read {OWNER_KEY_FILE_ENV} {}: {error}",
            path.display()
        ))
    })?;
    if private_key.trim().is_empty() {
        return Err(DeployError(format!(
            "{OWNER_KEY_FILE_ENV} must not be empty"
        )));
    }
    write_key(private_key.trim()).map(Some)
}

/// Materialize the target-scoped private key through the operator bootstrap
/// grant. An explicit owner-only `STADO_HOST_SSH_KEY_FILE` is the recovery
/// channel when that broker is unavailable. Private material never enters
/// argv, stdout, logs, or registry data.
pub async fn materialize(target: &str) -> Result<KeyFile, DeployError> {
    if let Some(key) = crate::deploy::host_channel::session_key(target) {
        return Ok(key);
    }
    if let Some(key) = owner_key_override()? {
        return Ok(key);
    }
    let id = item_id(target);
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| DeployError(error.to_string()))?;
    let client = Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(|error| DeployError(error.to_string()))?;
    // Ask for the one field this needs. A broker that requires a named field
    // refuses a whole-item read outright, and the refusal arrives as a bare
    // 400 that reads like a malformed request rather than a version skew --
    // which is how this call silently took every host command down with it.
    let private_key = client
        .read_field(&id, PRIVATE_KEY_FIELD)
        .await
        .map_err(|error| {
            if error.is_missing() {
                missing_key(&id, error)
            } else {
                unreachable_store(&id, target, &credentials.url, &credentials.consumer, error)
            }
        })?;
    let private_key = private_key
        .as_str()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault that answers "not there" is a missing key, and the repair is
    /// to put one in.
    #[test]
    fn an_absent_item_names_the_command_that_creates_one() {
        let refusal = missing_key(
            "stado-ssh-charless-mac-mini",
            SkarbiecError::MissingValue("stado-ssh-charless-mac-mini".to_string()),
        );
        assert!(refusal.0.contains("stado fleet key add"), "{}", refusal.0);
    }

    /// A vault that could not be reached is the circle this refusal exists
    /// for: the store may be the one the target itself serves.
    #[test]
    fn an_unreachable_store_names_the_store_the_host_and_the_way_out() {
        let refusal = unreachable_store(
            "stado-ssh-charless-mac-mini",
            "charless-mac-mini",
            "http://127.0.0.1:17602",
            "local-operator",
            SkarbiecError::Deployment("error sending request".to_string()),
        );
        assert!(
            refusal.0.contains("http://127.0.0.1:17602"),
            "{}",
            refusal.0
        );
        assert!(refusal.0.contains("charless-mac-mini"), "{}", refusal.0);
        assert!(refusal.0.contains("local-operator"), "{}", refusal.0);
        assert!(refusal.0.contains(OWNER_KEY_FILE_ENV), "{}", refusal.0);
        assert!(
            refusal.0.contains("only charless-mac-mini can hand out"),
            "the circle has to be said out loud: {}",
            refusal.0
        );
    }
}
