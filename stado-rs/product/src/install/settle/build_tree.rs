//! A desktop product's SwiftPM `.build` between builds.
//!
//! SwiftPM writes its build tree into the package, `<checkout>/.build`, and
//! every desktop product's own build script names that path, so a desktop
//! build runs with the tree there. Between builds it is kept in the
//! checkout's build area under Stado's home
//! ([`crate::common::runs::checkout_area`]), tagged with `CACHEDIR.TAG`: the
//! checkouts sit in `~/Documents`, which macOS keeps from the janitor, and a
//! tree left there was one no pass could ever reclaim. The next build moves
//! it back, so it compiles only what changed; a tree the janitor took under
//! disk pressure is built again whole.

use anyhow::{bail, Context, Result};
use std::{fs, io, path::Path};

/// The name SwiftPM gives its build tree inside a package.
pub const SWIFTPM_TREE: &str = ".build";

/// Where a checkout's SwiftPM tree is kept between builds, inside its build
/// area.
pub const KEPT_TREE: &str = "swiftpm";

/// Before a build: the tree kept at `kept` goes back into `checkout`. A
/// checkout that already holds one (a build run there by hand since) keeps
/// it: that tree is the newer, and the kept one is replaced when this build's
/// tree is put away.
pub fn bring_back(checkout: &Path, kept: &Path) -> Result<()> {
    let tree = checkout.join(SWIFTPM_TREE);
    if present(&tree)? || !present(kept)? {
        return Ok(());
    }
    fs::rename(kept, &tree).with_context(|| {
        format!(
            "moving the kept build tree {} back into {}",
            kept.display(),
            tree.display()
        )
    })
}

/// After a build: the checkout's `.build` moves to `kept`, replacing the tree
/// kept there before, and is tagged so the janitor may reclaim it.
pub fn put_away(checkout: &Path, kept: &Path) -> Result<()> {
    let tree = checkout.join(SWIFTPM_TREE);
    if !present(&tree)? {
        return Ok(());
    }
    if !tree.symlink_metadata()?.is_dir() {
        bail!(
            "{} is not a directory, so it is not a SwiftPM build tree; it was left where it is",
            tree.display()
        );
    }
    if present(kept)? {
        fs::remove_dir_all(kept)
            .with_context(|| format!("removing the earlier kept build tree {}", kept.display()))?;
    }
    let parent = kept
        .parent()
        .with_context(|| format!("{} has no parent directory", kept.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    fs::rename(&tree, kept).with_context(|| {
        format!(
            "moving the build tree {} to {} under Stado's home; both must be on one volume",
            tree.display(),
            kept.display()
        )
    })?;
    crate::common::tag_cache(kept, "stado product install")
}

/// Whether anything, a link included, is at `path`.
fn present(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}
