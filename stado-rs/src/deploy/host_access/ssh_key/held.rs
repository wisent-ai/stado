//! The last repair key the vault handed out for each host, kept owner-only
//! under the home for the day that vault cannot answer.

use std::path::PathBuf;

/// Where the last key the vault handed out for each host is kept, owner-only,
/// under the home: the vault that holds a host's repair key can be the broken
/// service on that very host, and the key that repairs it must not depend on
/// it.
const HELD_KEYS_DIR: &str = ".stado/host-keys";

/// `~/.stado/host-keys/<target>` under the home every Stado file is kept
/// under ([`crate::config_file::home`]); a home neither HOME nor the user
/// database names is the cause, where this process used to stop.
fn held_key_path(target: &str) -> Result<PathBuf, String> {
    Ok(crate::config_file::home()?.join(HELD_KEYS_DIR).join(target))
}

/// Keep the key the vault just answered, owner-only, whole and flushed with
/// its directory, for the day the vault cannot answer. A failure to keep it
/// is not a failure of this command, but it is said on stderr with the path
/// and cause: kept silently, the operator learned the repair key was missing
/// only on the day the vault was down.
#[cfg(unix)]
pub(super) fn hold_key(target: &str, private_key: &str) {
    use std::os::unix::fs::DirBuilderExt;
    let path = match held_key_path(target) {
        Ok(path) => path,
        Err(cause) => {
            eprintln!("the repair key for {target} is not held: {cause}");
            return;
        }
    };
    let Some(directory) = path.parent() else {
        return;
    };
    if let Err(error) = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(crate::primitives::file_mode::owner_only_directory())
        .create(directory)
    {
        eprintln!(
            "the repair key for {target} is not held: {} cannot be created: {error}",
            directory.display()
        );
        return;
    }
    if let Err(error) = crate::primitives::document::persist(&path, private_key.as_bytes()) {
        eprintln!(
            "the repair key for {target} is not held: {} cannot be written: {error}",
            path.display()
        );
    }
}

#[cfg(not(unix))]
pub(super) fn hold_key(_target: &str, _private_key: &str) {}

/// The key held from the last answer for `target`. No held file is `None`
/// without a word; a held file that is not an owner-only regular file, or
/// cannot be read, is also not used, and why is said on stderr with its path,
/// so a refused channel on the day the vault is down says why the repair key
/// did not open it.
pub(super) fn held_key(target: &str) -> Option<String> {
    match read_held_key(target) {
        Ok(key) => key,
        Err(cause) => {
            eprintln!("{cause}");
            None
        }
    }
}

fn read_held_key(target: &str) -> Result<Option<String>, String> {
    let path =
        held_key_path(target).map_err(|cause| format!("no held key for {target}: {cause}"))?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("held key {}: {error}", path.display())),
    };
    if !metadata.file_type().is_file() {
        return Err(format!("held key {} is not a regular file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if crate::primitives::file_mode::open_to_others(metadata.permissions().mode()) {
            return Err(format!(
                "held key {} is readable by others; it is not used",
                path.display()
            ));
        }
    }
    let key = std::fs::read_to_string(&path)
        .map_err(|error| format!("held key {}: {error}", path.display()))?;
    Ok((!key.trim().is_empty()).then(|| key.trim().to_string()))
}
