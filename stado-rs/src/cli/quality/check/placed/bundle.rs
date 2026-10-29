//! The committed source a placed check carries to its host: a Git bundle of
//! the revision, kept in the fleet store until the host has taken it.
//!
//! A bundle, not an archive of the tree, because the check reads Git: it
//! exports the revision itself and judges what it changes against the
//! published main. The bundle holds the revision's history and the
//! checkout's `origin/main`, so the host rebuilds the one repository the
//! check needs and runs exactly the check the checkout would.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

/// The branch a check judges a revision's changes against, as the checkout
/// tracks it.
const PUBLISHED_MAIN: &str = "refs/remotes/origin/main";

/// Where the bundle of `revision` of `product` waits in the fleet store.
pub(super) fn key(product: &str, revision: &str) -> String {
    format!("runs/quality/{product}/{revision}.bundle")
}

/// Stado's own area on the placing machine for one run's bundle,
/// `~/.stado/products/placed/<pid>-<revision>`: created here and removed
/// with [`remove`] once the bundle is in the store.
pub(super) fn area(revision: &str) -> Result<PathBuf, CmdError> {
    created(
        crate::config_file::expand_tilde("~/.stado/products/placed")
            .join(format!("{}-{revision}", std::process::id())),
    )
}

/// Stado's own area on the host for the product whose bundle is at `key`,
/// `~/.stado/products/placed/<product>`: one path per product, so every
/// check of it on this host builds with the same Cargo build directory
/// (the check's own, named from the repository path). A host runs one
/// placed check at a time (its job is exclusive), so what an ended run left
/// there is removed before the area is created again.
pub(super) fn host_area(key: &str) -> Result<PathBuf, CmdError> {
    let product = key
        .strip_prefix("runs/quality/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(product, _)| product)
        .filter(|product| !product.is_empty() && !product.contains(".."))
        .ok_or_else(|| {
            CmdError::usage(format!(
                "--bundle {key} is not a placed check's bundle key (runs/quality/<product>/<revision>.bundle)"
            ))
        })?;
    let area = crate::config_file::expand_tilde("~/.stado/products/placed").join(product);
    match std::fs::remove_dir_all(&area) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(
                CmdError::click(format!("cannot remove {}: {error}", area.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind())),
            )
        }
    }
    created(area)
}

fn created(area: PathBuf) -> Result<PathBuf, CmdError> {
    std::fs::create_dir_all(&area).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", area.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Ok(area)
}

/// Remove a run's [`area`].
pub(super) fn remove(area: &Path) -> Result<(), CmdError> {
    std::fs::remove_dir_all(area).map_err(|error| {
        CmdError::click(format!("cannot remove {}: {error}", area.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

fn git(directory: &Path, args: &[&str]) -> Result<String, CmdError> {
    let shown = format!("git -C {} {}", directory.display(), args.join(" "));
    let output = crate::wait::output(Command::new("git").arg("-C").arg(directory).args(args))
        .map_err(|cause| CmdError::click(format!("`{shown}` did not run: {cause}")))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "`{shown}` exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Bundle `revision` of `checkout` with the checkout's published main (its
/// `HEAD` when it tracks none) into `file`.
pub(super) fn create(checkout: &Path, revision: &str, file: &Path) -> Result<(), CmdError> {
    let head = if git(
        checkout,
        &["show-ref", "--verify", "--quiet", PUBLISHED_MAIN],
    )
    .is_ok()
    {
        PUBLISHED_MAIN
    } else {
        "HEAD"
    };
    let file = file.to_str().ok_or_else(|| {
        CmdError::click(format!("the bundle path {} is not UTF-8", file.display()))
    })?;
    git(
        checkout,
        &["bundle", "create", "--quiet", file, revision, head],
    )
    .map(drop)
}

/// Put `file` in the fleet store at `key`, unless a check of the same
/// revision already put it there.
pub(super) async fn upload(store: &JobStorage, key: &str, file: &Path) -> Result<(), CmdError> {
    store
        .upload_file_if_absent(key, file)
        .await
        .map(drop)
        .map_err(|error| {
            CmdError::from(error)
                .within(format!("put the source bundle at {key} in the fleet store"))
        })
}

/// The repository a host rebuilds from the bundle at `key`, under `area`:
/// every object the bundle holds, its published main as `origin/main`, and
/// the bundle named as `origin`, so the check reads what it changes against
/// main without fetching anything. The bundle leaves the store once the
/// host holds it.
pub(super) async fn unpack(
    store: &JobStorage,
    key: &str,
    revision: &str,
    area: &Path,
) -> Result<PathBuf, CmdError> {
    let bytes = store
        .read_bytes(key)
        .await
        .map_err(|error| CmdError::from(error).within(format!("read the source bundle {key}")))?
        .ok_or_else(|| {
            CmdError::refused(format!(
                "the fleet store holds no source bundle at {key}: a newer check of the product \
                 superseded this one, or it was already taken; place the check again with \
                 `stado quality check --place --revision {revision}`"
            ))
        })?;
    let file = area.join("source.bundle");
    std::fs::write(&file, bytes).map_err(|error| {
        CmdError::click(format!("cannot write {}: {error}", file.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let repository = area.join("repository");
    std::fs::create_dir_all(&repository).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", repository.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let bundle = file.to_string_lossy().into_owned();
    git(&repository, &["init", "--quiet"])?;
    let heads = git(&repository, &["bundle", "unbundle", &bundle])?;
    for line in heads.lines() {
        if let Some((object, PUBLISHED_MAIN)) = line.split_once(' ') {
            git(&repository, &["update-ref", PUBLISHED_MAIN, object])?;
            git(&repository, &["remote", "add", "origin", &bundle])?;
        }
    }
    git(
        &repository,
        &["cat-file", "-e", &format!("{revision}^{{commit}}")],
    )
    .map_err(|error| {
        error.within(format!(
            "the bundle at {key} does not hold commit {revision}"
        ))
    })?;
    store.delete_blob(key).await.map_err(|error| {
        CmdError::from(error).within(format!("remove the taken source bundle {key}"))
    })?;
    Ok(repository)
}
