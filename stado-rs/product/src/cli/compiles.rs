//! Which parsed `stado product` operations compile into a build directory the
//! disk janitor may reclaim, declared beside the operations themselves so
//! Stado's caller holds the janitor's cleanup lock for exactly those.
//!
//! An installation, an update, a Cargo build and every Swift operation
//! compile, resolve or restore into the package's build tree. Reading retained
//! Cargo results does not. The editor
//! server only answers from settings an index already wrote. Holding the
//! cleanup lock for its lifetime would prevent the janitor and installations
//! from ever acquiring their turn.

use anyhow::{Context, Result};
use clap::ArgMatches;
use std::path::{Path, PathBuf};

/// Whether `matches` (the `stado product` invocation) compiles into the
/// reclaimable build tree.
pub fn compiles(matches: &ArgMatches) -> bool {
    match matches.subcommand() {
        Some(("cargo", cargo)) => !matches!(
            cargo.get_one::<String>("operation"),
            Some(operation) if operation == "results"
        ),
        Some(("install" | "update", operation)) => !operation.get_flag("plan"),
        Some(("swift", _)) => true,
        _ => false,
    }
}

/// The catalog product a `cargo` or `swift` operation compiles, so Oko is
/// asked about that product's build rather than every product's: the one
/// whose `owner_repository` or a surface's `repository` is the GitHub origin
/// of the Git repository holding `--manifest-path` (`--package-path`), the
/// current directory when none is given. A tree that is no Git checkout,
/// has no GitHub origin or belongs to no catalogued product (a dependency,
/// a scratch tree) answers `None`, and Oko is asked about every product's
/// build, as before. The catalog is the committed one an installation reads
/// (its checkout's committed head, or `origin/main` when the head is behind):
/// a checkout's working-tree file, half written or older than the fields the
/// catalog now requires, refused every Cargo build of every product with
/// `ster names no star_repository`. A catalog that cannot be read is refused
/// naming it.
pub fn compiled_product(matches: &ArgMatches) -> Result<Option<String>> {
    let path = match matches.subcommand() {
        Some(("cargo", arguments)) => arguments.get_one::<String>("manifest-path"),
        Some(("swift", arguments)) => arguments.get_one::<String>("package-path"),
        _ => return Ok(None),
    };
    let start = match path {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().context("the current directory cannot be read")?,
    };
    // A manifest path names a file; a package path, or a manifest path that
    // does not exist yet, is asked from the directory it names or sits in.
    let directory = match start.is_dir() {
        true => start,
        false => start.parent().map(Path::to_path_buf).unwrap_or(start),
    };
    let Ok(root) = crate::source::git(&directory, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };
    let Some(identity) = crate::source::origin(Path::new(&root))
        .ok()
        .and_then(|remote| crate::source::repository(&remote))
    else {
        return Ok(None);
    };
    if let Some(workspace) = matches.get_one::<String>("workspace") {
        crate::common::use_workspace(PathBuf::from(workspace))?;
    }
    let runtime =
        crate::common::Runtime::new(matches.get_one::<String>("catalog").map(PathBuf::from))?;
    let (document, _) =
        crate::catalog::committed::for_installation(&runtime, "", None).with_context(|| {
            format!(
                "the committed catalog of {} cannot be read to find which product {identity} is",
                runtime.catalog.display()
            )
        })?;
    Ok(owning_product(&document, &identity))
}

/// The id of the catalog product that claims `repository` (`OWNER/REPO`, in
/// lower case), as its owner repository or a surface's.
fn owning_product(document: &serde_json::Value, repository: &str) -> Option<String> {
    let claims = |value: &serde_json::Value| {
        value
            .as_str()
            .is_some_and(|claimed| claimed.eq_ignore_ascii_case(repository))
    };
    document["products"].as_array()?.iter().find_map(|product| {
        let surfaces = product["surfaces"].as_array().into_iter().flatten();
        let claimed = claims(&product["owner_repository"])
            || surfaces
                .into_iter()
                .any(|surface| claims(&surface["repository"]));
        claimed
            .then(|| product["id"].as_str().map(str::to_owned))
            .flatten()
    })
}
