//! Explicit removal of every directory immediately below the local scratch root.
//! Loose files and links at that root stay; links inside a removed tree are
//! unlinked, never followed. This is an operator command, not a periodic sweeper.

mod filesystem;

use crate::providers::local::disk_cleanup::{free_bytes, safefs};
use serde::Serialize;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub const SCRATCH_ROOT: &str = ".stado/work";
const REPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchEntry {
    pub name: String,
    pub path: PathBuf,
    pub bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchFailure {
    pub path: PathBuf,
    pub operation: &'static str,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchReport {
    pub schema_version: u32,
    pub root: PathBuf,
    pub root_present: bool,
    pub applied: bool,
    pub directories: Vec<ScratchEntry>,
    pub removed: Vec<ScratchEntry>,
    pub failed: Vec<ScratchFailure>,
    pub remaining_directories: Vec<PathBuf>,
    pub apparent_bytes: i64,
    pub apparent_bytes_removed: i64,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    pub stray_files: usize,
    pub bytes_stray_files: i64,
}

impl ScratchReport {
    fn fail(&mut self, path: &Path, operation: &'static str, error: impl std::fmt::Display) {
        self.failed.push(ScratchFailure {
            path: path.into(),
            operation,
            error: error.to_string(),
        });
    }

    pub fn complete(&self) -> bool {
        self.failed.is_empty() && (!self.applied || self.remaining_directories.is_empty())
    }
}

pub fn sweep(apply: bool) -> ScratchReport {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut report = ScratchReport {
        schema_version: REPORT_VERSION,
        root: home
            .as_ref()
            .map(|path| path.join(SCRATCH_ROOT))
            .unwrap_or_default(),
        root_present: false,
        applied: apply,
        directories: Vec::new(),
        removed: Vec::new(),
        failed: Vec::new(),
        remaining_directories: Vec::new(),
        apparent_bytes: i64::default(),
        apparent_bytes_removed: i64::default(),
        free_bytes_before: None,
        free_bytes_after: None,
        stray_files: usize::default(),
        bytes_stray_files: i64::default(),
    };
    let Some(home) = home else {
        report.fail(Path::new("~"), "resolve home", "HOME is not set");
        return report;
    };
    let home = match home.canonicalize() {
        Ok(home) => home,
        Err(error) => {
            report.fail(&home, "resolve home", error);
            return report;
        }
    };
    report.root = home.join(SCRATCH_ROOT);
    let root = report.root.clone();
    let fd = match filesystem::open_root(&home) {
        Ok(Some(fd)) => fd,
        Ok(None) => return report,
        Err(error) => {
            report.fail(&root, "open scratch root without following links", error);
            return report;
        }
    };
    report.root_present = true;
    report.free_bytes_before = free_bytes(&home).ok();
    let entries = match filesystem::names(fd.as_raw_fd()) {
        Ok(entries) => entries,
        Err(error) => {
            report.fail(&root, "list scratch root", error);
            return report;
        }
    };
    for name in entries {
        let path = root.join(&name);
        let info = match safefs::fstatat_nofollow(fd.as_raw_fd(), &name) {
            Ok(info) => info,
            Err(error) => {
                report.fail(&path, "inspect entry", error);
                continue;
            }
        };
        if info.st_mode & nix::libc::S_IFMT != nix::libc::S_IFDIR {
            report.stray_files += 1;
            report.bytes_stray_files += info.st_size.max(0);
            continue;
        }
        let tree = match filesystem::Tree::open(fd.as_raw_fd(), name, info) {
            Ok(tree) => tree,
            Err(error) => {
                report.fail(&path, "open working directory", error);
                continue;
            }
        };
        let bytes = match tree.bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                report.fail(&path, "measure working directory", error);
                continue;
            }
        };
        let entry = ScratchEntry {
            name: tree.name.to_string_lossy().into_owned(),
            path: path.clone(),
            bytes,
        };
        report.apparent_bytes += bytes;
        report.directories.push(entry.clone());
        if apply {
            match tree.remove(fd.as_raw_fd()) {
                Ok(()) => {
                    report.apparent_bytes_removed += bytes;
                    report.removed.push(entry);
                }
                Err(error) => report.fail(&path, "remove working directory", error),
            }
        }
    }
    match filesystem::names(fd.as_raw_fd()) {
        Ok(entries) => {
            for name in entries {
                match safefs::fstatat_nofollow(fd.as_raw_fd(), &name) {
                    Ok(info) if info.st_mode & nix::libc::S_IFMT == nix::libc::S_IFDIR => {
                        report.remaining_directories.push(root.join(name))
                    }
                    Ok(_) => (),
                    Err(error) => report.fail(&root.join(name), "verify final entry", error),
                }
            }
        }
        Err(error) => report.fail(&root, "verify final directory list", error),
    }
    report.free_bytes_after = free_bytes(&home).ok();
    report
}
