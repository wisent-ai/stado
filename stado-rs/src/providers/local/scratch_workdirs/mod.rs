//! The working directories under `~/.stado/work` that no owner declares, and
//! their removal.
//!
//! Three names below that root belong to something: `jobs` holds queue job
//! trees and is swept by the `queue_workdirs` cleaner once the queue reports a
//! job terminal; `runs` and `run-signals` are
//! [`crate::deploy::host_run`]'s run area. Everything else directly under the
//! root is scratch an agent or an operator created by hand, and until this
//! module nothing in Stado could name it, size it, or remove it.
//!
//! That gap is measurable. On 2026-09-09 the unowned remainder on
//! `lukasz-macbook` was 292 GiB across 1696 directories, on a 1.8 TiB disk
//! with 33 GiB free — and the janitor called the host healthy, because its
//! `build_caches` cleaner was rooted at the source tree and no declared
//! cleaner reached this root at all. A root every agent is told to write to,
//! with no cleaner behind it, only ever grows.
//!
//! Removal is unconditional by the operator's instruction: these directories
//! are disposable, and anything that must survive belongs in a repository.
//! The sizing and removal walks are the ones the weles and Chromium cleaners
//! already share, so this adds no second judgement about symlinks.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::weles::tree_ops;

/// Scratch root, relative to the account that runs the agent.
pub const SCRATCH_ROOT: &str = ".stado/work";

/// Names directly under [`SCRATCH_ROOT`] that another part of Stado owns, and
/// which this sweep therefore never touches. Kept beside the sweep because a
/// name that moves out of this list becomes removable in the same edit.
pub const DECLARED_AREAS: [(&str, &str); 3] = [
    (
        "jobs",
        "queue job trees; swept by the queue_workdirs cleaner",
    ),
    ("runs", "host run area; owned by deploy::host_run"),
    ("run-signals", "host run signals; owned by deploy::host_run"),
];

/// One directory found directly under the scratch root.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchEntry {
    pub name: String,
    pub path: PathBuf,
    pub bytes: i64,
}

/// One directory an owner claims, reported so the total is explainable.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredArea {
    pub name: String,
    pub bytes: i64,
    pub owner: String,
}

/// What one removal did, or why it did not happen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchRemoval {
    pub path: PathBuf,
    pub bytes: i64,
    pub error: Option<String>,
}

/// The whole answer: what is there, what was left alone and to whom, and what
/// this pass removed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchReport {
    pub root: PathBuf,
    pub root_present: bool,
    pub applied: bool,
    pub undeclared: Vec<ScratchEntry>,
    pub declared: Vec<DeclaredArea>,
    pub removed: Vec<ScratchRemoval>,
    pub failed: Vec<ScratchRemoval>,
    pub bytes_undeclared: i64,
    pub bytes_removed: i64,
    /// Entries directly under the root that are not directories, with their
    /// total size. They are reported and never removed: this sweep is about
    /// working directories, and a loose file at the root is a different
    /// decision the operator has not made.
    pub stray_files: usize,
    pub bytes_stray_files: i64,
}

/// Canonical scratch root for this account, resolved the way the queue's own
/// root is: the physical home, then the relative components.
pub fn root() -> PathBuf {
    let home = crate::config_file::expand_tilde("~");
    let resolved = std::fs::canonicalize(&home).unwrap_or(home);
    resolved.join(SCRATCH_ROOT)
}

/// The owner of a name directly under the root, when one claims it.
pub fn owner(name: &str) -> Option<&'static str> {
    DECLARED_AREAS
        .iter()
        .find(|(area, _)| *area == name)
        .map(|(_, owner)| *owner)
}

/// Read the root and size everything directly under it, following no symlink
/// and crossing into no other owner's area.
///
/// Returns the undeclared directories, the declared areas, and the count and
/// size of entries that are not directories at all.
fn inventory(root: &Path) -> (Vec<ScratchEntry>, Vec<DeclaredArea>, usize, i64) {
    let mut undeclared = Vec::new();
    let mut declared = Vec::new();
    let mut stray_files = usize::default();
    let mut stray_bytes = i64::default();
    let Ok(entries) = std::fs::read_dir(root) else {
        return (undeclared, declared, stray_files, stray_bytes);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(info) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !info.is_dir() || info.file_type().is_symlink() {
            // A symlink is not a working directory, and removing one would
            // reach outside the root without saying so. A loose file is not
            // one either; both are counted so the root's total is explainable.
            stray_files += 1;
            stray_bytes += info.len() as i64;
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let bytes = tree_ops::dir_size(&path);
        match owner(&name) {
            Some(owner) => declared.push(DeclaredArea {
                name,
                bytes,
                owner: owner.to_string(),
            }),
            None => undeclared.push(ScratchEntry { name, path, bytes }),
        }
    }
    undeclared.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    declared.sort_by(|a, b| a.name.cmp(&b.name));
    (undeclared, declared, stray_files, stray_bytes)
}

/// Plan the sweep, and with `apply` perform it.
///
/// Without `apply` nothing is touched and `removed` is empty: the report is
/// then exactly what an operator would get by running it for real, which is
/// why there is one function and not two.
pub fn sweep(apply: bool) -> ScratchReport {
    let root = root();
    let root_present = root.is_dir();
    let (undeclared, declared, stray_files, bytes_stray_files) = inventory(&root);
    let bytes_undeclared = undeclared.iter().map(|entry| entry.bytes).sum();
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    let mut bytes_removed = i64::from(false);
    if apply {
        for entry in &undeclared {
            match tree_ops::remove_tree(&entry.path) {
                Ok(()) => {
                    bytes_removed += entry.bytes;
                    removed.push(ScratchRemoval {
                        path: entry.path.clone(),
                        bytes: entry.bytes,
                        error: None,
                    });
                }
                Err(error) => failed.push(ScratchRemoval {
                    path: entry.path.clone(),
                    bytes: entry.bytes,
                    error: Some(error.to_string()),
                }),
            }
        }
    }
    ScratchReport {
        root,
        root_present,
        applied: apply,
        undeclared,
        declared,
        removed,
        failed,
        bytes_undeclared,
        bytes_removed,
        stray_files,
        bytes_stray_files,
    }
}
