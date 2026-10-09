//! `stado product build-trees list|remove`: the rebuildable trees in the
//! workspace's checkouts, read and removed by the process that runs the
//! command.
//!
//! The checkouts sit in `~/Documents`, which macOS keeps from a program the
//! person at the Mac did not allow, and the host's janitor is such a program:
//! it cannot reclaim a build tree there under any disk pressure. This command
//! reads and removes them from the process it runs in — a terminal macOS lets
//! read the folder — and takes only what a build tool or Stado declared
//! regenerable: a directory carrying a valid `CACHEDIR.TAG`, the run areas
//! older Stado installs wrote into checkouts ([`find::STADO_RUN_AREAS`]), and
//! a desktop product's SwiftPM `.build`. A tree a build is writing now — it
//! holds Stado's run marker, SwiftPM's lock or Cargo's lock — is kept.

mod find;

use crate::{
    catalog,
    common::{emit, runs::gib, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// One tree and what this run did with it.
struct Row {
    checkout: PathBuf,
    tree: find::Tree,
    /// `reclaimable`, `in_use`, `removed` or `failed`.
    state: &'static str,
}

/// `stado product build-trees list|remove [--json]`.
pub fn run(operation: &str, json_output: bool, runtime: &Runtime) -> Result<i32> {
    let remove = match operation {
        "list" => false,
        "remove" => true,
        other => bail!("unknown build-trees operation {other}; use list or remove"),
    };
    let workspace = &runtime.workspace;
    let desktop = desktop_repositories(runtime)?;
    let checkouts = checkouts_in(workspace)?;
    let free_before = free(workspace)?;
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for checkout in &checkouts {
        let is_desktop = repository_of(checkout).is_some_and(|name| desktop.contains(&name));
        let (trees, unread) = find::trees(checkout, is_desktop)?;
        errors.extend(unread);
        for tree in trees {
            let state = if tree.holder.is_some() {
                "in_use"
            } else if !remove {
                "reclaimable"
            } else if let Err(error) = fs::remove_dir_all(&tree.path) {
                errors.push(format!("removing {}: {error}", tree.path.display()));
                "failed"
            } else {
                "removed"
            };
            rows.push(Row {
                checkout: checkout.clone(),
                tree,
                state,
            });
        }
    }
    let free_after = free(workspace)?;
    let bytes = |state: Option<&str>| -> u64 {
        rows.iter()
            .filter(|row| state.is_none_or(|state| row.state == state))
            .map(|row| row.tree.bytes)
            .sum()
    };
    let (total, in_use, removed) = (bytes(None), bytes(Some("in_use")), bytes(Some("removed")));
    let failed: Vec<&Row> = rows.iter().filter(|row| row.state == "failed").collect();
    if json_output {
        emit(&json!({
            "operation": operation,
            "workspace": workspace,
            "checkouts": checkouts.len(),
            "trees": rows.iter().map(|row| json!({
                "checkout": row.checkout,
                "path": row.tree.path,
                "declared_by": row.tree.declared_by,
                "bytes": row.tree.bytes,
                "state": row.state,
                "lock": row.tree.holder,
            })).collect::<Vec<_>>(),
            "bytes": total,
            "in_use_bytes": in_use,
            "removed_bytes": removed,
            "free_bytes_before": free_before,
            "free_bytes_after": free_after,
            "errors": errors,
        }))?;
    } else {
        for row in &rows {
            let lock = row
                .tree
                .holder
                .iter()
                .map(|lock| format!(", held by {}", lock.display()))
                .collect::<String>();
            println!(
                "{:<11} {:>8.1} GiB  {}  ({}{lock})",
                row.state,
                gib(row.tree.bytes),
                row.tree.path.display(),
                row.tree.declared_by,
            );
        }
        println!(
            "{} checkouts in {}: {:.1} GiB in rebuildable trees, {:.1} GiB held by a running \
             build, {:.1} GiB removed; {:.1} GiB free before, {:.1} GiB after",
            checkouts.len(),
            workspace.display(),
            gib(total),
            gib(in_use),
            gib(removed),
            gib(free_before),
            gib(free_after),
        );
        for error in &errors {
            eprintln!("{error}");
        }
    }
    if !failed.is_empty() {
        bail!(
            "build-tree removal incomplete: {} tree(s) under {} could not be removed; each is \
             named with the operating system's reason in errors",
            failed.len(),
            workspace.display()
        );
    }
    Ok(libc::EXIT_SUCCESS)
}

/// The free bytes of the volume holding `path`.
fn free(path: &Path) -> Result<u64> {
    fs2::available_space(path)
        .with_context(|| format!("reading the free space under {}", path.display()))
}

/// The git checkouts directly in the workspace, in name order.
fn checkouts_in(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut checkouts = Vec::new();
    for entry in fs::read_dir(workspace)
        .with_context(|| format!("reading the workspace {}", workspace.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading the workspace {}", workspace.display()))?;
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && path.join(".git").is_dir() {
            checkouts.push(path);
        }
    }
    checkouts.sort();
    Ok(checkouts)
}

/// The GitHub repository a checkout's origin names; a checkout without one
/// is no catalog product's.
fn repository_of(checkout: &Path) -> Option<String> {
    source::origin(checkout)
        .ok()
        .and_then(|origin| source::repository(&origin))
}

/// Every repository the catalog builds a desktop surface from.
fn desktop_repositories(runtime: &Runtime) -> Result<BTreeSet<String>> {
    let document = catalog::current(runtime)?;
    let products = document["products"]
        .as_array()
        .context("the catalog's products must be an array")?;
    Ok(products
        .iter()
        .filter_map(|product| product["installations"].as_array())
        .flatten()
        .filter(|recipe| recipe["surface"] == "desktop")
        .filter_map(|recipe| recipe["repository"].as_str())
        .map(str::to_ascii_lowercase)
        .collect())
}
