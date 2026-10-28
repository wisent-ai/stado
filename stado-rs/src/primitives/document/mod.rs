//! A JSON document Stado keeps in one file of its own: where it lives (the
//! variable that moves it, else `~/.stado/<file>`), read whole (no file is
//! `None`; one that cannot be read or parsed is the error naming it), and
//! written whole, staged beside it and renamed over it, with a staging file
//! a failed write leaves said beside the failure.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

/// `$<variable>` when it is set and not empty, else `~/.stado/<file>` under
/// the home every Stado file is kept under ([`crate::config_file::home`]:
/// HOME, else the user database); the error names `what`, the variable and
/// why no home was found when neither answers.
pub fn located(variable: &str, file: &str, what: &str) -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let home = crate::config_file::home()
        .map_err(|cause| format!("the {what} has no place: {variable} is not set and {cause}"))?;
    Ok(home.join(".stado").join(file))
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

/// Write `value`, the `what`, to `path` whole and durably ([`persist`]): a
/// reader meets the old document or the new one, two writers never write
/// into one staging file, and a host that loses power after the write still
/// holds the new document.
pub fn write(path: &Path, value: &impl Serialize, what: &str) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("the {what} cannot be written as JSON: {error}"))?;
    persist(path, &text).map_err(|error| format!("writing the {what} {}: {error}", path.display()))
}

/// Put `bytes` at `path` whole: written to a fresh file beside it, flushed to
/// the disk, renamed over it, and the directory flushed, so the rename itself
/// survives a crash. The staging file a failed write leaves is removed with
/// it. The one way Stado replaces a file it keeps; separate copies flushed
/// the file but not the directory, or neither, so a record written just
/// before a host lost power could come back as the old one or as nothing.
pub fn persist(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    persist_with_mode(
        path,
        bytes,
        crate::primitives::file_mode::owner_read_write(),
    )
}

/// [`persist`] for a file another account or program reads too: the file
/// takes `mode` ([`crate::primitives::file_mode`]) before it is renamed into
/// place, so no reader ever meets it with the owner-only bits it was created
/// with.
pub fn persist_with_mode(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    persist_staged(path, mode, Placing::Replace, |staged| {
        staged.write_all(bytes)
    })
    .map(drop)
}

/// [`persist_with_mode`] with the bytes streamed from the file at `source`
/// rather than held in memory: a program or archive member put at the place
/// readers attest or run it from, whole and durably.
pub fn persist_copy(path: &Path, source: &Path, mode: u32) -> std::io::Result<()> {
    persist_staged(path, mode, Placing::Replace, |staged| {
        let mut from = std::fs::File::open(source)?;
        std::io::copy(&mut from, staged).map(drop)
    })
    .map(drop)
}

/// [`persist_with_mode`] only where nothing is: the file is put in place by a
/// link the kernel refuses when `path` exists, so of two writers racing for
/// one record exactly one places it. `false` when `path` was already there,
/// left untouched; the caller decides whether what it holds is acceptable.
pub fn persist_if_absent(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<bool> {
    use std::io::Write;
    persist_staged(path, mode, Placing::IfAbsent, |staged| {
        staged.write_all(bytes)
    })
}

/// How a staged file takes its place: over what is there, or only where
/// nothing is.
#[derive(Clone, Copy)]
enum Placing {
    Replace,
    IfAbsent,
}

/// The staging, flush, placement and directory flush every writer above
/// shares; `fill` writes the content into the staging file. `false` only when
/// `IfAbsent` found `path` already there.
fn persist_staged(
    path: &Path,
    mode: u32,
    placing: Placing,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::other(format!(
            "{} names no directory to write into",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    fill(staged.as_file_mut())?;
    staged
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode))?;
    staged.as_file().sync_all()?;
    match placing {
        Placing::Replace => {
            staged.persist(path).map_err(|error| error.error)?;
        }
        Placing::IfAbsent => match staged.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(false)
            }
            Err(error) => return Err(error.error),
        },
    }
    std::fs::File::open(parent)?.sync_all()?;
    Ok(true)
}
