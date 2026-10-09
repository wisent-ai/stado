//! Paths, digests and the privileged file effects of the storage-root
//! transaction. A file the managed account cannot read is hashed, cloned and
//! handed back through `sudo -n`, and only inside the two live roots or the
//! transaction's own clone staging.

mod clone;
mod darwin;
mod inventory;
mod seal;

use std::fs;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{Context, Step};

pub(super) use clone::clone_file;
pub(super) use inventory::{
    complete_physical_inventory, item_path, object_paths, physical_inventory,
    validate_complete_inventory, validate_physical_checkpoint,
};
pub(super) use seal::{checkpoint_tree, seal_tree, set_tree_modes, validate_sealed_tree};

pub(super) const PRIVATE_FILE: u32 = 0o600;
const PRIVATE_DIRECTORY: u32 = 0o700;
/// A SHA-256 digest written as lowercase hexadecimal.
const SHA256_HEX_CHARACTERS: usize = 64;

pub(super) fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(super) fn join(root: &str, relative: &str) -> String {
    Path::new(root)
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn make_private_dirs(path: &str) -> Step<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIRECTORY)
        .create(path)
        .map_err(|error| format!("cannot create {path}: {error}"))
}

pub(super) fn fsync_dir(path: &str) -> Step<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync directory {path}: {error}"))
}

/// A directory that is itself a directory, not a link to one.
pub(super) fn real_directory(path: &str) -> bool {
    matches!(fs::symlink_metadata(path), Ok(info) if info.is_dir())
}

fn absolute(path: &str) -> PathBuf {
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    normal
}

/// Whether `path` lies strictly inside one of `roots`, after `..` is resolved.
fn path_within(path: &str, roots: &[&str]) -> bool {
    let candidate = absolute(path);
    roots.iter().any(|root| {
        let root = absolute(root);
        candidate != root && candidate.starts_with(&root)
    })
}

fn confined(path: &str, roots: &[&str], label: &str) -> Step<String> {
    if path_within(path, roots) {
        Ok(absolute(path).to_string_lossy().into_owned())
    } else {
        Err(format!("{label} escaped its captured transaction roots"))
    }
}

/// `sudo -n ARGUMENTS`, with the transaction's lock descriptor as its stdin
/// so the privileged step runs under the same held lock.
fn privileged(context: &Context, arguments: &[&str], label: &str) -> Step<Output> {
    let Some(lock) = context.lock_fd else {
        return Err(format!(
            "{label}: resident transaction lock descriptor is unavailable"
        ));
    };
    let duplicate = unsafe { nix::libc::dup(lock) };
    if duplicate < 0 {
        return Err(format!("{label}: {}", io::Error::last_os_error()));
    }
    // SAFETY: `duplicate` is a fresh descriptor this process owns.
    let stdin = Stdio::from(unsafe { OwnedFd::from_raw_fd(duplicate) });
    let output = crate::wait::output(
        &mut Command::new("/usr/bin/sudo")
            .arg("-n")
            .args(arguments)
            .stdin(stdin),
    )
    .map_err(|error| format!("{label}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.is_empty() { stdout } else { stderr };
        let last = detail.trim().lines().last().map(str::to_string);
        return Err(format!(
            "{label}: {}",
            last.as_deref().unwrap_or("privileged command failed")
        ));
    }
    Ok(output)
}

fn privileged_digest(context: &Context, path: &str) -> Step<String> {
    let (source, label) = if path_within(path, &[&context.primary, &context.backup]) {
        (
            confined(
                path,
                &[&context.primary, &context.backup],
                "privileged physical-root digest",
            )?,
            "cannot hash unreadable physical-root file",
        )
    } else if path_within(path, &[&context.staging]) {
        (
            confined(
                path,
                &[&context.staging],
                "privileged transaction-staging digest",
            )?,
            "cannot hash interrupted privileged clone",
        )
    } else {
        return Err("privileged digest escaped the live roots and transaction staging".to_string());
    };
    let output = privileged(
        context,
        &["/usr/bin/openssl", "dgst", "-sha256", "-r", &source],
        label,
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let encoded = stdout.split_whitespace().next().unwrap_or("");
    if encoded.len() != SHA256_HEX_CHARACTERS
        || !encoded
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
    {
        return Err("privileged confined digest has invalid output".to_string());
    }
    Ok(encoded.to_string())
}

fn recover_privileged_clone(context: &Context, destination: &str) -> Step<()> {
    let destination = confined(
        destination,
        &[&context.staging],
        "privileged clone recovery destination",
    )?;
    let info = fs::symlink_metadata(&destination)
        .map_err(|error| format!("cannot inspect privileged clone {destination}: {error}"))?;
    if !info.file_type().is_file() {
        return Err("privileged clone recovery found a non-regular staging entry".to_string());
    }
    privileged(
        context,
        &["/usr/bin/chflags", "nouchg,noschg", &destination],
        "cannot clear immutable flags on privileged clone",
    )?;
    let owner = format!(
        "{}:{}",
        nix::unistd::getuid().as_raw(),
        nix::unistd::getgid().as_raw()
    );
    privileged(
        context,
        &["/usr/sbin/chown", &owner, &destination],
        "cannot transfer privileged clone ownership",
    )?;
    Ok(())
}

fn privileged_clone(context: &Context, source: &str, destination: &str) -> Step<()> {
    let source = confined(
        source,
        &[&context.primary, &context.backup],
        "privileged copy-on-write clone source",
    )?;
    let destination = confined(
        destination,
        &[&context.staging],
        "privileged copy-on-write clone destination",
    )?;
    privileged(
        context,
        &["/bin/cp", "-c", "-p", &source, &destination],
        "privileged copy-on-write clone failed",
    )?;
    recover_privileged_clone(context, &destination)
}

pub(super) fn digest(context: &Context, path: &str) -> Step<String> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return privileged_digest(context, path);
        }
        Err(error) => return Err(format!("cannot hash {path}: {error}")),
    };
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|error| format!("cannot hash {path}: {error}"))?;
    Ok(hex::encode(hasher.finalize()))
}

/// `{"bytes", "sha256"}` of a regular file, or null when nothing is there.
pub(super) fn regular_identity(context: &Context, path: &str) -> Step<Value> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(format!("cannot inspect {path}: {error}")),
    };
    if !info.file_type().is_file() {
        return Err(format!("non-regular object: {path}"));
    }
    Ok(json!({"bytes": info.len(), "sha256": digest(context, path)?}))
}

/// The object's sidecar name under `.metadata`, relative to it.
pub(super) fn metadata_name(relative: &str) -> String {
    if relative.ends_with(".json") {
        relative.to_string()
    } else {
        format!("{relative}.json")
    }
}

/// Where an object's metadata sidecar lives; refuses a symlinked directory
/// on the way to it.
pub(super) fn metadata_path(root: &str, relative: &str) -> Step<String> {
    let candidate = join(&join(root, ".metadata"), &metadata_name(relative));
    let directory = parent_of(&candidate);
    let inside = Path::new(&directory)
        .strip_prefix(root)
        .map_err(|_| format!("metadata path leaves its root: {candidate}"))?;
    let mut current = PathBuf::from(root);
    for component in inside.components() {
        current.push(component);
        if fs::symlink_metadata(&current).is_ok_and(|info| info.file_type().is_symlink()) {
            return Err(format!(
                "symlinked metadata directory: {}",
                current.display()
            ));
        }
    }
    Ok(candidate)
}
