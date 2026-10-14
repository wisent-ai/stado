//! The one sizing walk and the one removal walk, shared with the clone
//! cleaner so both cleaners carry a single symlink judgement.

use std::io;
use std::path::{Path, PathBuf};

/// Python `_weles_dir_size`: recursive file-size total. os.walk
/// classifies with entry.is_dir() (FOLLOWS symlinks: a symlinked dir is
/// listed but, with followlinks=False, never recursed and never sized);
/// getsize also follows symlinks. Unreadable entries are left out of the
/// total and named on stderr, so a reclaim reporting fewer bytes says why.
///
/// Shared with [`super::super::chromium_clones`], which sizes the same shape of
/// thing — one shallow directory of files under a fixed root — and would
/// otherwise be a second walk with its own symlink judgement.
pub(in crate::providers::local::disk_cleanup) fn dir_size(path: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "disk cleanup: {} is not counted: {error}",
                    current.display()
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "disk cleanup: an entry of {} is not counted: {error}",
                        current.display()
                    );
                    continue;
                }
            };
            let child = entry.path();
            let read = entry
                .file_type()
                .and_then(|kind| Ok((kind, std::fs::metadata(&child)?)));
            let (kind, target) = match read {
                Ok(read) => read,
                Err(error) => {
                    eprintln!("disk cleanup: {} is not counted: {error}", child.display());
                    continue;
                }
            };
            if target.is_dir() {
                if !kind.is_symlink() {
                    stack.push(child);
                }
            } else {
                total += target.len() as i64;
            }
        }
    }
    total
}

/// Python `shutil.rmtree(entry.path)`: refuses a top-level symlink, never
/// follows symlinked directories, unlinks everything else. Errors abort
/// the removal and surface to the caller (Python's default onerror).
///
/// Shared with [`super::super::chromium_clones`] for the reason [`dir_size`] is:
/// two spellings of "delete this tree, refusing symlinks" would be two
/// safety models, and only one of them would be the tested one.
pub(in crate::providers::local::disk_cleanup) fn remove_tree(path: &Path) -> io::Result<()> {
    let info = std::fs::symlink_metadata(path)?;
    if info.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot call rmtree on a symbolic link",
        ));
    }
    if !info.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(path)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    for child in entries {
        let child_info = std::fs::symlink_metadata(&child)?;
        if child_info.is_dir() && !child_info.file_type().is_symlink() {
            remove_tree(&child)?;
        } else {
            std::fs::remove_file(&child)?;
        }
    }
    std::fs::remove_dir(path)
}
