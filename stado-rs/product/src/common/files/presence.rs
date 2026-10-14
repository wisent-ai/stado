//! Whether a path is there, told apart from whether it can be examined.
//!
//! `Path::exists` answers false for a path it cannot stat (a permission or
//! I/O fault), so an installed path, an evidence report or a release
//! directory read through it was taken as absent. Only a path that does not
//! exist is absent here; every other cause is the error naming the path.

use std::path::Path;

use anyhow::{Context, Result};

/// True when `path` exists, false when it does not; a path that cannot be
/// examined is an error naming the path and the cause.
pub fn present(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("{} cannot be examined", path.display())),
    }
}
