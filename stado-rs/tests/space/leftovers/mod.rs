//! What a release install leaves on a host, reclaimed for real inside the
//! fixture's own home.
//!
//! `stado release install-local` writes an attestation copy and a retained
//! archive under `~/.stado/releases/<product>/<version>/<platform>/`, the
//! installed coordinate in `~/.stado/bin/<product>.release-version`, and one
//! dated backup of the replaced binary beside it. The `delivery_leftovers`
//! stage keeps the installed version, the newest version and the newest
//! backup, and takes the stale rest.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::fixture::{only_stage, reported_paths, Host, AUDIT_LOG, TARGET};

/// Old enough for the stage's age gate, which refuses anything younger than
/// a day.
pub(super) const AGED_DAYS: u64 = 3;

/// A delivered version's tree, as install-local leaves it, aged past the gate.
pub(super) fn deliver(host: &Host, version: &str) -> PathBuf {
    let tree = host.under_home(&format!(".stado/releases/stado/{version}"));
    let platform = tree.join("darwin-arm64");
    fs::create_dir_all(&platform).expect("create the delivered version tree");
    fs::write(platform.join("stado"), b"attestation copy\n").expect("write the attestation copy");
    fs::write(
        platform.join("stado-reader-convergence.tar.gz"),
        b"retained archive\n",
    )
    .expect("write the retained archive");
    age(&tree);
    tree
}

/// A dated backup of the installed binary, aged past the gate.
pub(super) fn backup(host: &Host, stamp: &str) -> PathBuf {
    let path = host.under_home(&format!(".stado/bin/stado.release-backup-{stamp}"));
    fs::write(&path, format!("binary before {stamp}\n")).expect("write the dated backup");
    age(&path);
    path
}

pub(super) fn age(path: &Path) {
    let aged = SystemTime::now() - Duration::from_secs(AGED_DAYS * 24 * 60 * 60);
    File::open(path)
        .expect("open the leftover to age it")
        .set_times(FileTimes::new().set_accessed(aged).set_modified(aged))
        .expect("age the leftover past the gate");
}

pub(super) fn assert_inside(root: &Path, paths: &[String]) {
    let root = root.to_string_lossy().to_string();
    for path in paths {
        assert!(
            path.starts_with(&root),
            "the preview named {path}, which is outside this test's tempdir {root}; refusing to apply"
        );
    }
}


mod installed;
mod uncoordinated;
