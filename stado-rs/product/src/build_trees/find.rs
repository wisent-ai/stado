//! Finding the rebuildable trees in one checkout, what they hold and whether
//! a build is writing them now.

use anyhow::{Context, Result};
use std::{
    collections::VecDeque,
    fs::{self, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// The directories inside a checkout that Stado's own builds wrote before
/// those builds moved to the checkout's build area under Stado's home
/// ([`crate::common::runs::checkout_area`]). Each holds runs no later build
/// reads.
pub const STADO_RUN_AREAS: &[&str] = &[
    ".wisent-output/install",
    ".wisent-output/local-install",
    ".wisent-output/cargo-install",
    ".build/local-install",
    ".build/package-install",
    ".build/wisent-source",
];

/// The lock files a build holds while it writes a tree: Stado's run marker
/// ([`crate::common::runs`]), SwiftPM's workspace lock and Cargo's build
/// directory lock.
const LOCKS: &[&str] = &[".in-use", ".lock", ".cargo-lock"];

/// The file that declares its directory regenerable
/// (https://bford.info/cachedir/).
const TAG: &str = "CACHEDIR.TAG";

/// What may follow the signature on the tag's first line: its end.
const LINE_END: &[u8] = b"\n";

/// One rebuildable tree, why it is one, and what it holds.
pub struct Tree {
    pub path: PathBuf,
    /// `cachedir_tag`, `stado_run_area` or `desktop_build_tree`.
    pub declared_by: &'static str,
    /// The lengths of the files in the tree.
    pub bytes: u64,
    /// The lock a build holds in it right now, when one does.
    pub holder: Option<PathBuf>,
}

/// Every rebuildable tree of `checkout`, outermost only: the trees Stado's
/// builds wrote there ([`STADO_RUN_AREAS`]), its SwiftPM `.build` when the
/// checkout is a desktop product's, and every directory carrying a valid
/// `CACHEDIR.TAG`. Symbolic links are never followed, another volume or
/// another account's directory is never entered, and `.git` is never walked.
/// A directory the walk cannot read is named in the returned errors and the
/// walk goes on.
pub fn trees(checkout: &Path, desktop: bool) -> Result<(Vec<Tree>, Vec<String>)> {
    let root = fs::symlink_metadata(checkout)
        .with_context(|| format!("reading the checkout {}", checkout.display()))?;
    let mut found: Vec<Tree> = Vec::new();
    let mut errors = Vec::new();
    let desktop_tree = desktop.then(|| (checkout.join(".build"), "desktop_build_tree"));
    let declared = desktop_tree.into_iter().chain(
        STADO_RUN_AREAS
            .iter()
            .map(|area| (checkout.join(area), "stado_run_area")),
    );
    for (path, declared_by) in declared {
        if covered(&found, &path) {
            continue;
        }
        match fs::symlink_metadata(&path) {
            Ok(info) if own_directory(&info, &root) => {
                found.push(measure(path, declared_by, &root, &mut errors))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("reading {}: {error}", path.display())),
        }
    }
    let mut frontier = VecDeque::from([checkout.to_path_buf()]);
    while let Some(directory) = frontier.pop_front() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!("reading {}: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!("reading {}: {error}", directory.display()));
                    continue;
                }
            };
            let path = entry.path();
            if entry.file_name() == ".git" || covered(&found, &path) {
                continue;
            }
            // `DirEntry::metadata` does not follow a symbolic link on unix.
            let info = match entry.metadata() {
                Ok(info) => info,
                Err(error) => {
                    errors.push(format!("reading {}: {error}", path.display()));
                    continue;
                }
            };
            if !own_directory(&info, &root) {
                continue;
            }
            match tagged(&path, &root) {
                Ok(true) => found.push(measure(path, "cachedir_tag", &root, &mut errors)),
                Ok(false) => frontier.push_back(path),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
    }
    Ok((found, errors))
}

/// Whether `path` is one of the trees already found or inside one.
fn covered(found: &[Tree], path: &Path) -> bool {
    found.iter().any(|tree| path.starts_with(&tree.path))
}

/// A real directory (no link) on the checkout's volume, owned by the
/// checkout's owner.
fn own_directory(info: &Metadata, root: &Metadata) -> bool {
    info.file_type().is_dir() && info.dev() == root.dev() && info.uid() == root.uid()
}

/// Whether `directory` carries a `CACHEDIR.TAG` its owner wrote whose first
/// line is the standard's signature.
fn tagged(directory: &Path, root: &Metadata) -> Result<bool> {
    let path = directory.join(TAG);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        // A link where the tag should be is somebody else's statement, not
        // the build tool's, and authorizes nothing.
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let info = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if !info.file_type().is_file() || info.uid() != root.uid() {
        return Ok(false);
    }
    let signature = crate::common::CACHEDIR_SIGNATURE.as_bytes();
    let mut head = Vec::new();
    let mut reader = file.take(signature.len() as u64);
    reader
        .read_to_end(&mut head)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut end = Vec::new();
    reader
        .into_inner()
        .take(LINE_END.len() as u64)
        .read_to_end(&mut end)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(head == signature && (end.is_empty() || end == LINE_END))
}

fn measure(
    path: PathBuf,
    declared_by: &'static str,
    root: &Metadata,
    errors: &mut Vec<String>,
) -> Tree {
    let bytes = contents(&path, root, errors);
    let holder = holder(&path);
    Tree {
        path,
        declared_by,
        bytes,
        holder,
    }
}

/// The lengths of every entry in `tree`, links counted as themselves.
fn contents(tree: &Path, root: &Metadata, errors: &mut Vec<String>) -> u64 {
    let mut total = 0;
    let mut pending = vec![tree.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!("measuring {}: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(info) = entry.metadata() else {
                errors.push(format!("measuring {}", entry.path().display()));
                continue;
            };
            if info.file_type().is_dir() {
                if info.dev() == root.dev() {
                    pending.push(entry.path());
                }
            } else {
                total += info.len();
            }
        }
    }
    total
}

/// The first lock a build holds in `tree`: at its top, in a directory below
/// it (a Stado run, a Cargo profile) or one level further (a Cargo profile
/// under a target triple).
fn holder(tree: &Path) -> Option<PathBuf> {
    let mut directories = vec![tree.to_path_buf()];
    for child in subdirectories(tree) {
        directories.extend(subdirectories(&child));
        directories.push(child);
    }
    directories
        .iter()
        .flat_map(|directory| LOCKS.iter().map(move |lock| directory.join(lock)))
        .find(|lock| held(lock))
}

fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.metadata().is_ok_and(|info| info.file_type().is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// Whether a process holds the lock file at `path`: a shared lock this
/// process cannot take is a build's exclusive one.
fn held(path: &Path) -> bool {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file.try_lock_shared().is_err(),
        Err(_) => false,
    }
}
