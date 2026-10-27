//! What SwiftPM built in a desktop package's `.build`, and only that.
//!
//! A checkout keeps its own scratch (review stagings, journey runs,
//! qualification areas) in the same `.build` as SwiftPM's output, so the
//! directory is never one rebuildable tree, and `swift package clean` is no
//! way to reclaim it either: it removes every member of the scratch directory
//! except SwiftPM's dependency clones and its state file, the checkout's
//! scratch included.
//!
//! SwiftPM says itself which member it built into: the configuration link it
//! keeps at the top of a scratch directory (`debug -> <triple>/debug`) names,
//! as its first component, the per-triple build directory beside it. Those
//! directories are the rebuildable trees; nothing a link does not name is.
//! The `.build` is one scratch directory, and so is each member of it that
//! carries SwiftPM's state file (an editor's index build kept inside it).

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

/// SwiftPM's record of a scratch directory's resolved packages, written at
/// its top.
const SWIFTPM_STATE: &str = "workspace-state.json";

/// Every build directory SwiftPM's configuration links name in `build` and
/// in each scratch directory inside it, each once. The first read that fails
/// is the error, naming the path and the operating system's reason.
pub fn built(build: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found: Vec<PathBuf> = Vec::new();
    for scratch in scratch_directories(build)? {
        for directory in linked_directories(&scratch)? {
            if !found.contains(&directory) {
                found.push(directory);
            }
        }
    }
    Ok(found)
}

/// `build` and each of its members that carries SwiftPM's state file.
fn scratch_directories(build: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = vec![build.to_path_buf()];
    for (path, info) in members(build)? {
        if !info.is_dir() {
            continue;
        }
        let state = path.join(SWIFTPM_STATE);
        match fs::symlink_metadata(&state) {
            Ok(read) if read.is_file() => found.push(path),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reading {}: {error}", state.display())),
        }
    }
    Ok(found)
}

/// The directory beside each symbolic link at the top of `scratch` that the
/// link's relative target starts with.
fn linked_directories(scratch: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for (path, info) in members(scratch)? {
        if !info.file_type().is_symlink() {
            continue;
        }
        let target =
            fs::read_link(&path).map_err(|error| format!("reading the link {}: {error}", path.display()))?;
        if let Some(Component::Normal(first)) = target.components().next() {
            let directory = scratch.join(first);
            match fs::symlink_metadata(&directory) {
                Ok(beside) if beside.is_dir() => found.push(directory),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("reading {}: {error}", directory.display())),
            }
        }
    }
    Ok(found)
}

/// The members of `directory` with their own metadata, links not followed.
fn members(directory: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>, String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("reading {}: {error}", directory.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("reading {}: {error}", directory.display()))?;
        let info = entry
            .metadata()
            .map_err(|error| format!("reading {}: {error}", entry.path().display()))?;
        found.push((entry.path(), info));
    }
    Ok(found)
}
