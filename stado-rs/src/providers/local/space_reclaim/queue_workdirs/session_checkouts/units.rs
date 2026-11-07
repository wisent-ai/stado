//! Whether a launchd or systemd unit on this host names a path in a session
//! checkout: a service started from it keeps it.

use std::io;
use std::path::{Path, PathBuf};

use crate::deploy::service::{LAUNCHD_UNIT_DIRECTORIES, SYSTEMD_UNIT_DIRECTORIES};

/// The word the unit directory lists spell the account's home with.
const HOME_WORD: &str = "$HOME/";

/// A launchd or systemd unit definition on this host that names a path in
/// `checkout`: the unit starts that service again even while it is down, so
/// the checkout stays. `Err` names a unit directory or file that could not
/// be read, since absence was then never proven. Links in the unit
/// directories point at definitions listed elsewhere and are not read.
pub(super) fn unit_naming(home: &Path, checkout: &Path) -> Result<Option<PathBuf>, String> {
    let needle = checkout.to_string_lossy();
    for directory in LAUNCHD_UNIT_DIRECTORIES
        .iter()
        .chain(SYSTEMD_UNIT_DIRECTORIES.iter())
    {
        let directory = match directory.strip_prefix(HOME_WORD) {
            Some(relative) => home.join(relative),
            None => PathBuf::from(directory),
        };
        if let Some(unit) = naming_in(&directory, &needle)? {
            return Ok(Some(unit));
        }
    }
    Ok(None)
}

/// The first unit definition at or below `directory` whose text holds
/// `needle`.
fn naming_in(directory: &Path, needle: &str) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the unit directory {} cannot be read: {error}",
                directory.display()
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "an entry of the unit directory {} cannot be read: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            format!("the unit entry {} cannot be read: {error}", path.display())
        })?;
        if kind.is_dir() {
            if let Some(unit) = naming_in(&path, needle)? {
                return Ok(Some(unit));
            }
        } else if kind.is_file() {
            let text = std::fs::read(&path)
                .map_err(|error| format!("the unit {} cannot be read: {error}", path.display()))?;
            if String::from_utf8_lossy(&text).contains(needle) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}
