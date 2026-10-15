//! A JSON document Stado keeps in one file of its own: where it lives (the
//! variable that moves it, else `~/.stado/<file>`), read whole (no file is
//! `None`; one that cannot be read or parsed is the error naming it), and
//! written whole, staged beside it and renamed over it, with a staging file
//! a failed write leaves said beside the failure.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

/// `$<variable>` when it is set and not empty, else `~/.stado/<file>`; the
/// error names `what` and both places when neither is set.
pub fn located(variable: &str, file: &str, what: &str) -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("the {what} has no place: neither {variable} nor HOME is set"))?;
    Ok(PathBuf::from(home).join(".stado").join(file))
}

/// The `what` at `path`; `None` when no file is there.
pub fn read<T: DeserializeOwned>(path: &Path, what: &str) -> Result<Option<T>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading the {what} {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("the {what} {} is not a {what}: {error}", path.display()))
}

/// Write `value`, the `what`, to `path` whole: staged beside it under this
/// process's id, then renamed over it, so a reader meets the old document or
/// the new one and two writers never write into one staging file.
pub fn write(path: &Path, value: &impl Serialize, what: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    let text = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("the {what} cannot be written as JSON: {error}"))?;
    let staged = path.with_extension(format!("json.{}.writing", std::process::id()));
    if let Err(error) = std::fs::write(&staged, text) {
        let failure = format!("writing {}: {error}", staged.display());
        return Err(super::staging::failed_with(failure, &staged));
    }
    std::fs::rename(&staged, path).map_err(|error| {
        let failure = format!("writing {}: {error}", path.display());
        super::staging::failed_with(failure, &staged)
    })
}
