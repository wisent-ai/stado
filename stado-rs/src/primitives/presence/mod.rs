//! Whether a path is there, told apart from whether it can be examined.
//!
//! `Path::exists` answers false for a path it cannot stat (a permission or
//! I/O fault), so a job's output directory read through it was taken as
//! absent and its artifacts were never uploaded. Only a path that does not
//! exist is absent here; every other cause is an error of the same kind that
//! names the path.

use std::io;
use std::path::Path;

/// True when `path` exists, false when it does not; a path that cannot be
/// examined is an error naming the path and the cause.
pub fn present(path: &Path) -> io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{} cannot be examined: {error}", path.display()),
        )),
    }
}
