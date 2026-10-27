//! The one way a Stado command rewrites the config file it read.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::Value;

/// Replace the config file at `path` with `document`, pretty-printed with a
/// final newline, whole and flushed with its directory
/// ([`crate::primitives::document::persist_with_mode`]), keeping the
/// permission bits the file already has. Every caller has just read the file
/// at `path`, so a mode that cannot be read is the error. Separate copies
/// each wrote a fixed sibling (`.setting`, `.unsetting`, `.database-setting`,
/// `.migrating-identities`) that two writers shared, never flushed it, and
/// kept the old mode only when reading it happened to succeed.
pub(crate) fn replace(path: &Path, document: &Value) -> std::io::Result<()> {
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(document).map_err(std::io::Error::other)?
    );
    let mode = std::fs::metadata(path)?.permissions().mode()
        & crate::primitives::file_mode::permission_bits();
    crate::primitives::document::persist_with_mode(path, body.as_bytes(), mode)
}
