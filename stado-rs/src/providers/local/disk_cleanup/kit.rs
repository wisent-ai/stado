//! Shared fixtures for the disk-cleanup test suite: fabricated temp
//! `$HOME` trees (HF cache layouts, weles recordings), registry/policy
//! JSON builders, mtime backdating, and a `run_with_lock` driver.
//! Everything runs as the unprivileged test user and NEVER touches the
//! real `$HOME`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{acquire_lock, epoch_now, run_with_lock, CleanupReport};

/// A tempdir playing the role of `$HOME` (fully canonicalized: on macOS
/// `/var` is a symlink to `/private/var`, and the janitor compares
/// canonical paths).
pub struct TempHome {
    _dir: tempfile::TempDir,
    pub home: PathBuf,
}

impl TempHome {
    pub fn new() -> TempHome {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = std::fs::canonicalize(dir.path()).expect("canonical home");
        TempHome { _dir: dir, home }
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }
}

/// utimensat(AT_FDCWD, ..., AT_SYMLINK_NOFOLLOW): set a path's mtime in
/// seconds since epoch without following symlinks (test-only unsafe:
/// AT_FDCWD is a valid pseudo-descriptor by definition).
pub fn set_mtime(path: &Path, epoch_secs: i64) {
    use std::os::fd::BorrowedFd;
    use nix::sys::time::TimeValLike;
    let ts = nix::sys::time::TimeSpec::seconds(epoch_secs);
    let cwd = unsafe { BorrowedFd::borrow_raw(nix::libc::AT_FDCWD) };
    nix::sys::stat::utimensat(cwd, path, &ts, &ts, nix::sys::stat::UtimensatFlags::NoFollowSymlink)
        .unwrap_or_else(|e| panic!("backdate {}: {e}", path.display()));
}

/// Recursively backdate a tree `age_secs` into the past (children first;
/// the walk itself touches nothing).
pub fn backdate_tree(path: &Path, age_secs: i64) {
    let target = now_epoch() - age_secs;
    backdate_to(path, target);
}

fn backdate_to(path: &Path, epoch_secs: i64) {
    let info = std::fs::symlink_metadata(path).expect("lstat");
    if info.is_dir() && !info.file_type().is_symlink() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            backdate_to(&entry.expect("entry").path(), epoch_secs);
        }
    }
    set_mtime(path, epoch_secs);
}

/// Current epoch seconds.
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

/// A registry-v2 document naming `hostname` as a local target with the
/// given disk_cleanup policy object.
pub fn registry_json(hostname: &str, disk_cleanup: Value) -> Value {
    json!({
        "schema_version": 2,
        "targets": [{
            "name": hostname,
            "kind": "local",
            "disk_cleanup": disk_cleanup,
        }],
    })
}

/// A valid disk_cleanup policy. `low_gb`/`target_gb` of 1_000_000 /
/// 2_000_000 make pressure unconditionally active on any real disk.
pub fn policy_json(mode: &str, cleaners: Value) -> Value {
    json!({
        "mode": mode,
        "check_interval_seconds": 60,
        "low_free_gb": 1_000_000,
        "target_free_gb": 2_000_000,
        "max_bytes_per_pass": 1073741824,
        "max_items_per_pass": 100,
        "max_scan_items": 100000,
        "cleaners": cleaners,
    })
}

/// The default HF cleaner config (min_age at the registry floor).
pub fn hf_cleaner() -> Value {
    json!({"min_age_seconds": 3600})
}

/// The default weles cleaner config (min_age at the registry floor).
pub fn weles_cleaner() -> Value {
    json!({"min_age_seconds": 86400})
}

/// Drive one full cleanup pass against the fabricated home and registry,
/// exactly like `run_cleanup_once` but with the canonical registry
/// document injected (no GCS).
pub fn run_pass(
    th: &TempHome,
    registry: Value,
    hostname: &str,
    active_slot_count: i64,
    force: bool,
) -> Value {
    let state_dir = super::ensure_state_dir(&th.home).expect("state dir");
    let lock = acquire_lock(&state_dir).expect("lock io").expect("lock busy");
    let report = CleanupReport::base(active_slot_count, hostname);
    let mut logs: Vec<String> = Vec::new();
    run_with_lock(
        &th.home,
        &state_dir,
        lock,
        Ok(registry),
        report,
        std::time::Instant::now(),
        epoch_now(),
        force,
        &mut |line| logs.push(line.to_string()),
    )
}

/// Fabricate an HF hub layout with one repository. Returns
/// (repo_dir, commit, blob_names).
///
/// Layout (huggingface_hub's on-disk cache):
///   hub/.locks/models--org--name/<commit>.lock
///   hub/models--org--name/blobs/<sha...>
///   hub/models--org--name/refs/main            (contains commit)
///   hub/models--org--name/snapshots/<commit>/file1.txt -> ../../blobs/<b1>
///   hub/models--org--name/snapshots/<commit>/sub/file2.txt -> ../../../blobs/<b2>
pub fn make_hf_repo(home: &Path, name: &str, commit: &str, blobs: &[(&str, &[u8])]) -> PathBuf {
    let hub = home.join(".cache/huggingface/hub");
    let repo = hub.join(name);
    let locks = hub.join(".locks").join(name);
    std::fs::create_dir_all(&locks).unwrap();
    std::fs::write(locks.join(format!("{commit}.lock")), b"").unwrap();
    std::fs::create_dir_all(repo.join("blobs")).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::create_dir_all(repo.join("snapshots").join(commit).join("sub")).unwrap();
    std::fs::write(repo.join("refs").join("main"), commit).unwrap();
    for (blob, content) in blobs {
        std::fs::write(repo.join("blobs").join(blob), content).unwrap();
    }
    std::os::unix::fs::symlink(
        format!("../../blobs/{}", blobs[0].0),
        repo.join("snapshots").join(commit).join("file1.txt"),
    )
    .unwrap();
    if blobs.len() > 1 {
        std::os::unix::fs::symlink(
            format!("../../../blobs/{}", blobs[1].0),
            repo.join("snapshots").join(commit).join("sub").join("file2.txt"),
        )
        .unwrap();
    }
    repo
}

/// Fabricate a weles recordings root with one run directory.
pub fn make_weles_run(home: &Path, run: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = home.join("weles/recordings").join(run);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
    dir
}

/// Write a valid v1 upload proof into the run directory.
pub fn write_upload_proof(run_dir: &Path, uploaded_at_epoch: i64, file_count: i64) {
    let stamp = chrono::DateTime::from_timestamp(uploaded_at_epoch, 0)
        .expect("timestamp")
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    std::fs::write(
        run_dir.join(".uploaded.json"),
        json!({"version": 1, "file_count": file_count, "uploaded_at": stamp}).to_string(),
    )
    .unwrap();
}
