//! Read every Cargo execution receipt retained in a checkout's Stado build
//! area. This is the same evidence directory `execute_revision` writes, not a
//! second build index that could disagree with it.
//!
//! `execute_revision` names each receipt's directory by its invocation, a
//! UUID. The same `cargo` area also holds the installation's Cargo cache
//! (`<platform>/<lock digest>/<revision>/<recipe>`), which is no receipt;
//! read as one, its missing `result.json` refused every receipt read, and
//! with it every supervised send that reads compiler charges.
//!
//! One entry that cannot be read never refuses the list: a receipt that
//! cannot be read or decoded and an area that cannot be listed are each
//! named under `unread` with the cause, and every other receipt is
//! answered. A directory that is no receipt (the installation's platform
//! cache) is named under `skipped`, so `unread` holds only what should have
//! been a receipt and could not be read.
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::common::Runtime;

/// The receipts read from one cargo area, every receipt of it that could not
/// be read, and every entry that is no receipt, each with why.
#[derive(Default)]
struct Read {
    results: Vec<Value>,
    unread: Vec<Value>,
    skipped: Vec<Value>,
}

impl Read {
    fn unread(&mut self, path: &Path, cause: String) {
        self.unread.push(json!({ "path": path, "cause": cause }));
    }
}

fn entries(area: &Path, read: &mut Read) {
    let entries = match fs::read_dir(area) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            return read.unread(
                area,
                format!("the Cargo receipt area cannot be listed: {error}"),
            )
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                read.unread(
                    area,
                    format!("an entry of the Cargo receipt area cannot be listed: {error}"),
                );
                continue;
            }
        };
        let directory = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {}
            Ok(_) => continue,
            Err(error) => {
                read.unread(&directory, format!("its type cannot be read: {error}"));
                continue;
            }
        }
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
        {
            read.skipped.push(json!({ "path": directory,
                "cause": "not a receipt: a receipt directory is named by its invocation's UUID" }));
            continue;
        }
        let path = directory.join("result.json");
        // An invocation writes its result when it ends; until then, or when
        // it ended without writing one, it is listed as unfinished.
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                read.results.push(json!({ "path": path, "state": "unfinished",
                    "error": "the invocation has written no result.json: it has not ended, or it ended without one" }));
                continue;
            }
            Err(error) => {
                read.unread(&path, format!("the receipt cannot be read: {error}"));
                continue;
            }
        };
        let report: Value = match serde_json::from_slice(&bytes) {
            Ok(report) => report,
            Err(error) => {
                read.unread(&path, format!("the receipt is not JSON: {error}"));
                continue;
            }
        };
        read.results.push(
            json!({ "path": path, "state": report["state"], "operation": report["operation"],
            "sources": report["sources"], "evidence": report["evidence"],
            "failedOperation": report["failed_operation"], "error": report["error"] }),
        );
    }
}

pub(super) fn list(runtime: &Runtime, requested: &Path) -> Result<Value> {
    let (manifest, checkout, repository) = super::sources::manifest(runtime, requested)?;
    let area = crate::common::runs::checkout_area(&runtime.home, &checkout)?.join("cargo");
    let mut read = Read::default();
    entries(&area, &mut read);
    Ok(
        json!({ "repository": repository, "manifest": manifest, "results": read.results,
        "unread": read.unread, "skipped": read.skipped }),
    )
}

/// Every canonical checkout in the selected workspace, without opening a
/// manifest or starting Cargo. The retained receipt remains the only source
/// of state, location and pinned revision. A selected workspace that does
/// not exist holds no checkout, so it has no receipt: the list is empty and
/// names the absent workspace under `unread`, as any area it cannot list.
pub(super) fn all(runtime: &Runtime) -> Result<Value> {
    let workspace = match runtime.workspace.canonicalize() {
        Ok(workspace) => workspace,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut read = Read::default();
            read.unread(
                &runtime.workspace,
                format!("the selected workspace does not exist, so it holds no checkout: {error}"),
            );
            return Ok(
                json!({ "workspace": runtime.workspace, "results": read.results,
                "unread": read.unread, "skipped": read.skipped }),
            );
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading canonical workspace {}",
                    runtime.workspace.display()
                )
            })
        }
    };
    let mut read = Read::default();
    for entry in fs::read_dir(&workspace)
        .with_context(|| format!("listing canonical checkouts under {}", workspace.display()))?
    {
        let entry = entry.with_context(|| {
            format!("listing canonical checkouts under {}", workspace.display())
        })?;
        if !entry
            .file_type()
            .with_context(|| format!("inspecting checkout {}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let area = crate::common::runs::checkout_area(&runtime.home, &entry.path())?.join("cargo");
        entries(&area, &mut read);
    }
    read.results
        .sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    read.unread
        .sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    read.skipped
        .sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(
        json!({ "workspace": workspace, "results": read.results, "unread": read.unread,
        "skipped": read.skipped }),
    )
}
