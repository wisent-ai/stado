//! The Swift package half of the source check: no target of a Swift package
//! in the exported tree holds two Swift sources of one file name.
//!
//! SwiftPM names each object file of a target after its source's file name,
//! in one build directory per target, so two `Models.swift` in different
//! folders of one target are two producers of one object and the target does
//! not build. The names are compared folded to lowercase: on a
//! case-insensitive filesystem `models.swift.o` and `Models.swift.o` are one
//! file. The tree is the commit's export, which holds tracked files only, so
//! no `.build` directory is walked.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

// SwiftPM's package manifest and the default root of its targets' sources:
// https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Usage.md#creating-a-package
const MANIFEST: &str = "Package.swift";
const TARGETS_ROOT: &str = "Sources";
const SWIFT_EXTENSION: &str = "swift";

/// The directories under `directory`, symbolic links excluded.
fn subdirectories(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
            .path();
        if path.is_dir() && !path.is_symlink() {
            found.push(path);
        }
    }
    Ok(found)
}

/// Every directory under `directory` holding a Swift package manifest.
fn packages(directory: &Path, into: &mut Vec<PathBuf>) -> Result<(), String> {
    if directory.join(MANIFEST).is_file() {
        into.push(directory.to_path_buf());
    }
    for child in subdirectories(directory)? {
        packages(&child, into)?;
    }
    Ok(())
}

/// Every Swift source under `directory`.
fn swift_sources(directory: &Path, into: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
            .path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            swift_sources(&path, into)?;
        } else if path.extension() == Some(OsStr::new(SWIFT_EXTENSION)) {
            into.push(path);
        }
    }
    Ok(())
}

/// One line per file name two or more Swift sources of one target share,
/// naming the target and every source holding it.
pub(super) fn colliding(tree: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    packages(tree, &mut found)?;
    let mut lines = Vec::new();
    for package in &found {
        let root = package.join(TARGETS_ROOT);
        if !root.is_dir() {
            continue;
        }
        for target in subdirectories(&root)? {
            let mut sources = Vec::new();
            swift_sources(&target, &mut sources)?;
            let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
            for source in sources {
                // `swift_sources` keeps only paths with a `.swift` extension,
                // so each has a file name; one that is not UTF-8 cannot be
                // compared and is said, never skipped.
                let name = source
                    .file_name()
                    .and_then(OsStr::to_str)
                    .ok_or_else(|| {
                        format!("{} has a file name that is not UTF-8", source.display())
                    })?
                    .to_lowercase();
                by_name.entry(name).or_default().push(source);
            }
            for (name, holders) in by_name {
                match holders.as_slice() {
                    [] | [_] => {}
                    _ => lines.push(format!(
                        "target {} compiles {name} from {}",
                        target.display(),
                        holders
                            .iter()
                            .map(|holder| holder.display().to_string())
                            .collect::<Vec<_>>()
                            .join(" and ")
                    )),
                }
            }
        }
    }
    Ok(lines)
}
