//! Removing a file Stado wrote beside its target and no longer needs: a
//! staging file whose write or rename failed.
//!
//! A removal that fails leaves the file on disk, and a long-running process
//! writes the next staging file under the same name with `create_new`, which
//! is then refused by the leftover's existence rather than by its real cause.
//! So the failure is said, never dropped; a file that is already gone is not
//! a failure.

use std::path::Path;

/// Remove `path`. `Some` is the sentence that says the file is still there
/// and why; `None` when it is gone, including when it never existed.
pub fn remove(path: &Path) -> Option<String> {
    match std::fs::remove_file(path) {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!("{} is left behind: {error}", path.display())),
    }
}

/// `error`, followed by the sentence of a staging file its removal left.
pub fn failed_with(error: String, staging: &Path) -> String {
    match remove(staging) {
        None => error,
        Some(left) => format!("{error}; {left}"),
    }
}
