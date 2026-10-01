//! The last repair key the vault handed out for each host, kept owner-only
//! under `$HOME` for the day that vault cannot answer.

#[cfg(unix)]
use std::io::Write;
use std::path::PathBuf;

/// Where the last key the vault handed out for each host is kept, owner-only,
/// under `$HOME`: the vault that holds a host's repair key can be the broken
/// service on that very host, and the key that repairs it must not depend on
/// it.
const HELD_KEYS_DIR: &str = ".stado/host-keys";

fn held_key_path(target: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(HELD_KEYS_DIR).join(target))
}

/// Keep the key the vault just answered, owner-only, for the day the vault
/// cannot answer. A failure to keep it is not a failure of this command.
#[cfg(unix)]
pub(super) fn hold_key(target: &str, private_key: &str) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let Some(path) = held_key_path(target) else {
        return;
    };
    let Some(directory) = path.parent() else {
        return;
    };
    if std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .is_err()
    {
        return;
    }
    let staged = path.with_extension("staged");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staged)
        .and_then(|mut file| file.write_all(private_key.as_bytes()));
    if written.is_ok() {
        let _ = std::fs::rename(&staged, &path);
    } else {
        let _ = std::fs::remove_file(&staged);
    }
}

#[cfg(not(unix))]
pub(super) fn hold_key(_target: &str, _private_key: &str) {}

/// The key held from the last answer for `target`, when it is an owner-only
/// regular file.
pub(super) fn held_key(target: &str) -> Option<String> {
    let path = held_key_path(target)?;
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let key = std::fs::read_to_string(&path).ok()?;
    (!key.trim().is_empty()).then(|| key.trim().to_string())
}
