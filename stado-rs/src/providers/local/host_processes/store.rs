//! The last per-process reading this host took, kept so the next reading is
//! measured from it whichever process takes it.

use std::io::ErrorKind;
use std::path::PathBuf;

use super::Reading;

/// Where the reading is kept: in the account's Stado state, beside the
/// other records a host keeps about itself, under the home every Stado file
/// is kept under ([`crate::config_file::home`]).
fn path() -> Result<PathBuf, String> {
    let home = crate::config_file::home().map_err(|cause| {
        format!("there is no Stado state directory to keep the process reading in: {cause}")
    })?;
    Ok(home
        .join(".stado")
        .join("state")
        .join("host-processes.json"))
}

/// The reading kept last, or `None` when this host has kept none yet.
pub(super) fn previous() -> Result<Option<Reading>, String> {
    let path = path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "the kept process reading {} does not parse: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "the kept process reading {} cannot be read: {error}",
            path.display()
        )),
    }
}

/// Keep `reading` in place of the last one, by one rename.
pub(super) fn keep(reading: &Reading) -> Result<(), String> {
    let path = path()?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "the process reading path {} has no directory",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let text = serde_json::to_string(reading)
        .map_err(|error| format!("the process reading does not serialize: {error}"))?;
    crate::targets::write_atomic(&path, &text).map_err(|error| {
        format!(
            "cannot keep the process reading at {}: {error}",
            path.display()
        )
    })
}
