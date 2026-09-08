//! Where one product's rollout state lives, the advisory lock that guards it,
//! the exact bytes it is committed as, and the identity it is refused unless
//! it carries.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;

use super::records::{HostReleaseState, STATE_SCHEMA};
use crate::release_control::ReleaseTargetPolicy;

/// The rollout state document one product keeps on one host.
///
/// Spelled once because `stado release quarantine` names this file over the
/// registry SSH channel while the agent opens it locally, and a command that
/// reads `<product>.json` from its own second spelling is a command that reads
/// a file no agent writes.
pub fn host_state_path(state_dir: &str, product: &str) -> String {
    format!("{state_dir}/{product}.json")
}

fn state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    PathBuf::from(host_state_path(&target.state_dir, product))
}

pub(crate) fn proxy_state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    Path::new(&target.state_dir).join(format!("{product}-proxy.json"))
}

/// A non-blocking exclusive advisory lock on one file in a release state
/// directory.
///
/// Crate-visible with a caller-supplied stem because the unit-image revisit
/// pass needs one lock per HOST over its whole observe -> restart -> record
/// sequence, and this is already the shape of that lock: same `fs2` advisory
/// mode, same `O_NOFOLLOW`, same `WouldBlock` means another holder rather than
/// a failure. A second implementation beside it would be a second answer to
/// "is somebody already doing this".
pub(crate) struct StateLock {
    file: File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// `Ok(None)` when another process holds it. The caller decides what a busy
/// lock means; nothing here waits.
pub(crate) fn acquire_state_lock(state_dir: &str, stem: &str) -> Result<Option<StateLock>, String> {
    let state_dir = Path::new(state_dir);
    std::fs::create_dir_all(state_dir).map_err(|error| {
        format!(
            "cannot create release state directory {}: {error}",
            state_dir.display()
        )
    })?;
    let path = state_dir.join(format!("{stem}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("cannot open release lock {}: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| format!("cannot inspect release lock {}: {error}", path.display()))?
        .is_file()
    {
        return Err(format!(
            "release lock is not a regular file: {}",
            path.display()
        ));
    }
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(StateLock { file })),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || matches!(
                    error.raw_os_error(),
                    Some(code)
                        if code == nix::libc::EACCES || code == nix::libc::EAGAIN
                ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!(
            "cannot acquire release lock {}: {error}",
            path.display()
        )),
    }
}

pub(crate) fn acquire_product_reconcile_lock(
    target: &ReleaseTargetPolicy,
    product: &str,
) -> Result<Option<StateLock>, String> {
    acquire_state_lock(&target.state_dir, &format!("{product}.reconcile"))
}

/// The exact bytes one document is committed as: compact JSON and one trailing
/// newline.
fn document_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode release state: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The bytes [`atomic_json`] would commit for one rollout state document.
///
/// `stado release quarantine clear` rewrites this file from off-host, and what
/// it sends has to be what this agent would have written: the digest the host
/// is asked to verify after the write is taken over exactly these bytes, so an
/// encoding that drifted from the agent's own fails that check instead of
/// quietly leaving two shapes of the same document in the fleet.
pub fn state_document_bytes(state: &HostReleaseState) -> Result<Vec<u8>, String> {
    document_bytes(state)
}

pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("state path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create state directory {}: {error}",
            parent.display()
        )
    })?;
    let staging = parent.join(format!(".state-{}", uuid::Uuid::new_v4().simple()));
    let bytes = document_bytes(value)?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| {
                format!("cannot create state staging {}: {error}", staging.display())
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                format!("cannot write state staging {}: {error}", staging.display())
            })?;
    }
    std::fs::rename(&staging, path)
        .map_err(|error| format!("cannot commit release state {}: {error}", path.display()))
}

/// Decode one rollout state document and refuse it unless it is the one that
/// was asked for.
///
/// Lifted out of [`load_state`] because `stado release quarantine` reads the
/// same document back over the registry SSH channel and then rewrites it.
/// These three checks are all that stands between a mistyped host name and a
/// state file overwritten with another host's rollout, so both readers make
/// them from one place rather than from two copies that agree today. `origin`
/// is whatever the caller can show an operator: a local path, or the remote
/// path it read.
pub fn parse_state_document(
    payload: &[u8],
    product: &str,
    target_name: &str,
    origin: &str,
) -> Result<HostReleaseState, String> {
    let state: HostReleaseState = serde_json::from_slice(payload)
        .map_err(|error| format!("invalid release state {origin}: {error}"))?;
    if state.schema_version != STATE_SCHEMA
        || state.product != product
        || state.target != target_name
    {
        return Err(format!("release state identity mismatch at {origin}"));
    }
    Ok(state)
}

pub(crate) fn load_state(
    target: &ReleaseTargetPolicy,
    product: &str,
    target_name: &str,
) -> Result<HostReleaseState, String> {
    let path = state_path(target, product);
    if !path.exists() {
        return Ok(HostReleaseState::new(product, target_name));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("cannot read release state {}: {error}", path.display()))?;
    parse_state_document(&bytes, product, target_name, &path.display().to_string())
}

pub(crate) fn save_state(
    target: &ReleaseTargetPolicy,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    state.updated_at = Utc::now();
    atomic_json(&state_path(target, &state.product), state)
}
