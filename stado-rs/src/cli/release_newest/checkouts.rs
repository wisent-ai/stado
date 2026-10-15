//! Which checkouts of a workspace declare a product. `stado release newest`
//! and `stado release catalog sync` both read the workspace through it.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;
use crate::primitives::presence::present;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// Every checkout under `root` that declares a product, by its directory, in
/// path order.
///
/// A release declaration belongs to the checkout that holds it, so the walk
/// stops at a directory that declares a product and at any other Git
/// checkout: a dependency clone or a build tree inside it is not another
/// product the operator checked out. Outside a checkout it passes over
/// hidden directories and the build caches that tag themselves with
/// `CACHEDIR.TAG` (https://bford.info/cachedir/). A path the walk cannot
/// examine is an error naming it, never a directory without a declaration.
pub fn declaring_checkouts(root: &Path) -> Result<Vec<PathBuf>, CmdError> {
    let mut found = Vec::new();
    scan(root, &mut found)?;
    found.sort();
    Ok(found)
}

fn scan(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    if present(&directory.join(PRODUCT_MANIFEST))? {
        found.push(directory.to_path_buf());
        return Ok(());
    }
    if present(&directory.join(".git"))? || present(&directory.join("CACHEDIR.TAG"))? {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(directory)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        scan(&entry.path(), found)?;
    }
    Ok(())
}
