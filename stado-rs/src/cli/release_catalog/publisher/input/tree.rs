//! A committed repository tree as the build reads it: the commit's files, the
//! files of every submodule at the commit the tree records for it, and every
//! symlink inside the tree replaced by the file or folder it names.
//!
//! `git archive` alone carries neither: a submodule is an empty folder in it
//! and a symlink stays a symlink, which the release worker's extraction
//! refuses ("release entry is not a regular file or directory"). A Swift
//! package that wraps a C library through symlinks into its submodules
//! (espeak-ng-spm) could therefore be neither pinned nor mounted. Here each
//! repository is archived by Git at its recorded commit into one staging
//! folder below the checkout's build directory, the links are resolved
//! inside that folder, and the folder is archived under the input's name. A
//! link that leaves the tree, or names nothing, is refused by name.

use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::cli::CmdError;

use super::git;

/// The object type `git ls-tree` prints for a submodule's recorded commit.
const GITLINK_TYPE: &str = "commit";

pub(super) fn export(
    source: &Path,
    commit: &str,
    name: &str,
    paths: &[String],
    scratch: &Path,
) -> Result<tempfile::NamedTempFile, CmdError> {
    std::fs::create_dir_all(scratch)?;
    let staging = tempfile::tempdir_in(scratch)?;
    let root = staging.path().join(name);
    materialize(source, commit, &root, paths)?;
    resolve_links(&root)?;
    let archive = tempfile::NamedTempFile::new_in(scratch)?;
    let mut builder = tar::Builder::new(GzEncoder::new(archive.reopen()?, Compression::default()));
    builder.mode(tar::HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    builder.append_dir_all(name, &root)?;
    builder.into_inner()?.finish()?;
    staging.close()?;
    Ok(archive)
}

/// `repository` at `commit` (only `paths` of it when any are named), and each
/// submodule it records, unpacked under `destination`.
fn materialize(
    repository: &Path,
    commit: &str,
    destination: &Path,
    paths: &[String],
) -> Result<(), CmdError> {
    let mut arguments = vec!["archive", "--format=tar", commit];
    if !paths.is_empty() {
        arguments.push("--");
        arguments.extend(paths.iter().map(String::as_str));
    }
    let bytes = git(repository, &arguments)?;
    std::fs::create_dir_all(destination)?;
    let mut archive = tar::Archive::new(bytes.as_slice());
    archive.set_preserve_permissions(true);
    archive.unpack(destination)?;
    for (path, recorded) in submodules(repository, commit)? {
        let inside_kept = paths.is_empty()
            || paths
                .iter()
                .any(|kept| path == *kept || path.starts_with(&format!("{kept}/")));
        if !inside_kept {
            if let Some(kept) = paths
                .iter()
                .find(|kept| kept.starts_with(&format!("{path}/")))
            {
                return Err(CmdError::usage(format!(
                    "--path {kept:?} lies inside submodule {path}; name the submodule's folder {path:?} instead"
                )));
            }
            continue;
        }
        let checkout = repository.join(&path);
        git(
            &checkout,
            &["cat-file", "-e", &format!("{recorded}^{{commit}}")],
        )
        .map_err(|_| {
            CmdError::usage(format!(
                "submodule {path} of {} records commit {recorded}, which {} does not hold; \
                 run `git submodule update --init --recursive` in {}",
                repository.display(),
                checkout.display(),
                repository.display()
            ))
        })?;
        materialize(&checkout, &recorded, &destination.join(&path), &[])?;
    }
    Ok(())
}

/// Every submodule `commit` records: its path and the commit it pins.
fn submodules(repository: &Path, commit: &str) -> Result<Vec<(String, String)>, CmdError> {
    let listing = git(repository, &["ls-tree", "-r", "-z", commit])?;
    let mut found = Vec::new();
    for entry in listing.split(|byte| *byte == b'\0') {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = meta
            .split_whitespace()
            .skip_while(|field| *field != GITLINK_TYPE);
        if let (Some(_), Some(recorded)) = (fields.next(), fields.next()) {
            found.push((path.to_string(), recorded.to_string()));
        }
    }
    Ok(found)
}

/// Replace every symlink below `root` by what it names, refusing a link that
/// leaves `root` or names nothing.
fn resolve_links(root: &Path) -> Result<(), CmdError> {
    let boundary = std::fs::canonicalize(root)?;
    let mut links = Vec::new();
    collect_links(root, &mut links)?;
    for link in links {
        let relative = link.strip_prefix(root).map_err(|error| {
            CmdError::click(format!(
                "{} is not below {}: {error}",
                link.display(),
                root.display()
            ))
        })?;
        let target = std::fs::canonicalize(&link).map_err(|error| {
            CmdError::usage(format!(
                "symlink {} names nothing the tree holds: {error}",
                relative.display()
            ))
        })?;
        if !target.starts_with(&boundary) {
            return Err(CmdError::usage(format!(
                "symlink {} leaves the tree: it names {}",
                relative.display(),
                target.display()
            )));
        }
        std::fs::remove_file(&link)?;
        copy_followed(&target, &link, &boundary)?;
    }
    Ok(())
}

/// The symlinks below `folder`, without entering a linked folder.
fn collect_links(folder: &Path, links: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        let kind = std::fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() {
            links.push(path);
        } else if kind.is_dir() {
            collect_links(&path, links)?;
        }
    }
    Ok(())
}

/// Copy `source` to `destination`, following links, every file read staying
/// inside `boundary`.
fn copy_followed(source: &Path, destination: &Path, boundary: &Path) -> Result<(), CmdError> {
    let resolved = std::fs::canonicalize(source)?;
    if !resolved.starts_with(boundary) {
        return Err(CmdError::usage(format!(
            "{} leaves the tree: it names {}",
            source.display(),
            resolved.display()
        )));
    }
    if resolved.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(&resolved)? {
            let entry = entry?;
            copy_followed(
                &entry.path(),
                &destination.join(entry.file_name()),
                boundary,
            )?;
        }
    } else {
        std::fs::copy(&resolved, destination)?;
    }
    Ok(())
}
