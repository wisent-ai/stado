//! Asynchronous fleet-staging flush helpers for the local agent.
//!
//! Rust owns rotation, locking and child lifecycle. The detached child imports
//! only the external `wisent` job library's Hugging Face writer; it does not
//! import the retired Python `stado`/`wisent_compute` orchestrator. Successful
//! uploads remove the rotated directory, while every exit releases the PID
//! lock so a later agent tick can retry failed directories.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

/// Minimal child program around wisent's upload primitive. The import fallback
/// spans the package's old and current module layouts without depending on a
/// Python Stado module.
const FLUSH_RUNNER: &str = r#"
import os, shutil, sys

flush_dir, lock_path = sys.argv[-2:]
try:
    if not (os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")):
        from huggingface_hub import get_token
        token = get_token()
        if token:
            os.environ["HF_TOKEN"] = token
    try:
        from wisent.core.reading.modules.utilities.data.sources.hf.hf_writers import flush_staging_dir
    except ImportError:
        from wisent.scripts.activations.hf_writers import flush_staging_dir
    flush_staging_dir(flush_dir)
    shutil.rmtree(flush_dir)
finally:
    try:
        os.unlink(lock_path)
    except FileNotFoundError:
        pass
"#;

/// Python `_pid_live`: kill(pid, 0). EPERM counts as dead, matching
/// Python's broad `except OSError`.
pub fn pid_live(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// Python `_active_flush`: live pid in the lock file -> true; stale lock
/// is removed; unreadable/unparsable lock -> false.
pub fn active_flush(lock_path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(lock_path) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    if pid_live(pid) {
        return true;
    }
    let _ = std::fs::remove_file(lock_path);
    false
}

/// Pick the oldest rotated flush dir, or rotate `staging` into a fresh
/// one. Python the candidates/else branch of `spawn_fleet_flush`. Returns
/// None when there is nothing to flush (staging missing or empty).
pub(crate) fn pick_or_rotate(
    staging: &Path,
    flush_root: &Path,
) -> std::io::Result<Option<PathBuf>> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(flush_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort();
    if let Some(first) = candidates.into_iter().next() {
        return Ok(Some(first));
    }
    let non_empty = staging.is_dir()
        && std::fs::read_dir(staging)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
    if !non_empty {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let flush_dir = flush_root.join(format!("flush_{stamp}_{}", std::process::id()));
    // shutil.move: same-parent rename (flush_root sits next to staging).
    std::fs::rename(staging, &flush_dir)?;
    // New jobs write to a freshly recreated staging dir while the rotated
    // directory uploads.
    std::fs::create_dir_all(staging)?;
    Ok(Some(flush_dir))
}

/// Rotate a staging dir and flush it in a background process.
/// Python `spawn_fleet_flush`.
///
/// Returns Ok(true) when a background flush is active or was started. The
/// caller can keep admitting GPU jobs because new jobs write to a freshly
/// recreated fleet_staging directory while the rotated directory uploads.
pub fn spawn_fleet_flush(
    fleet_staging: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> std::io::Result<bool> {
    spawn_fleet_flush_with(&super::python_bin(), fleet_staging, log_fn)
}

/// [`spawn_fleet_flush`] with an explicit interpreter (tests pass `true`,
/// which ignores the child-program arguments).
pub fn spawn_fleet_flush_with(
    python: &str,
    fleet_staging: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> std::io::Result<bool> {
    let staging = fleet_staging;
    let name = staging
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let flush_root = staging.with_file_name(format!("{name}.async_flushes"));
    let log_root = staging.with_file_name(format!("{name}.flush_logs"));
    std::fs::create_dir_all(&flush_root)?;
    std::fs::create_dir_all(&log_root)?;
    let lock_path = flush_root.join(".active_pid");
    if active_flush(&lock_path) {
        return Ok(true);
    }
    let Some(flush_dir) = pick_or_rotate(staging, &flush_root)? else {
        return Ok(false);
    };

    let log_name = flush_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_root.join(format!("{log_name}.log")))?;
    let err_file = log_file.try_clone()?;
    let child = std::process::Command::new(python)
        .args(["-c", FLUSH_RUNNER])
        .arg(&flush_dir)
        .arg(&lock_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(err_file))
        // start_new_session=True: the flush survives the agent.
        .process_group(0)
        .spawn()?;
    let pid = child.id();
    std::fs::write(&lock_path, pid.to_string())?;
    log_fn(&format!(
        "started async fleet staging flush pid={pid} dir={}",
        flush_dir.display()
    ));
    // Detached like the Python Popen: never waited on. Dropping the Child
    // handle leaves the process running (it is re-parented to init).
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pid that is definitely dead: spawn `true` and reap it.
    fn dead_pid() -> i32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        pid
    }

    fn layout(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let staging = dir.join("fleet_staging");
        let flush_root = dir.join("fleet_staging.async_flushes");
        let log_root = dir.join("fleet_staging.flush_logs");
        (staging, flush_root, log_root)
    }

    #[test]
    fn pid_liveness() {
        assert!(pid_live(std::process::id() as i32));
        assert!(!pid_live(dead_pid()));
    }

    #[test]
    fn active_flush_lock_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(".active_pid");
        // Missing lock.
        assert!(!active_flush(&lock));
        // Unparsable lock.
        std::fs::write(&lock, "not-a-pid").unwrap();
        assert!(!active_flush(&lock));
        // Live pid (ourselves) holds the lock.
        std::fs::write(&lock, std::process::id().to_string()).unwrap();
        assert!(active_flush(&lock));
        // Stale pid is treated as inactive AND the lock is removed.
        std::fs::write(&lock, dead_pid().to_string()).unwrap();
        assert!(!active_flush(&lock));
        assert!(!lock.exists());
    }

    #[test]
    fn rotation_moves_staging_and_recreates_it() {
        let dir = tempfile::tempdir().unwrap();
        let (staging, flush_root, _) = layout(dir.path());
        std::fs::create_dir_all(&flush_root).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("shard.bin"), b"data").unwrap();

        let flush_dir = pick_or_rotate(&staging, &flush_root).unwrap().unwrap();
        assert!(flush_dir.parent() == Some(flush_root.as_path()));
        assert!(flush_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("flush_"));
        assert_eq!(std::fs::read(flush_dir.join("shard.bin")).unwrap(), b"data");
        // Fresh staging dir exists and is empty.
        assert!(staging.is_dir());
        assert!(std::fs::read_dir(&staging).unwrap().next().is_none());

        // A second rotation reuses the pending flush dir without rotating.
        assert_eq!(
            pick_or_rotate(&staging, &flush_root).unwrap().unwrap(),
            flush_dir
        );

        // Nothing pending and an empty staging -> nothing to flush.
        std::fs::remove_dir_all(&flush_dir).unwrap();
        assert!(pick_or_rotate(&staging, &flush_root).unwrap().is_none());
        // Missing staging dir -> nothing to flush.
        std::fs::remove_dir_all(&staging).unwrap();
        assert!(pick_or_rotate(&staging, &flush_root).unwrap().is_none());
    }

    #[test]
    fn spawn_rotates_spawns_and_writes_lock() {
        let dir = tempfile::tempdir().unwrap();
        let (staging, flush_root, log_root) = layout(dir.path());
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("shard.bin"), b"data").unwrap();

        let mut lines: Vec<String> = Vec::new();
        let started =
            spawn_fleet_flush_with("true", &staging, &mut |l: &str| lines.push(l.to_string()))
                .unwrap();
        assert!(started);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("started async fleet staging flush pid="),
            "{lines:?}"
        );

        // One rotated dir with the payload; staging recreated empty.
        let rotated: Vec<PathBuf> = std::fs::read_dir(&flush_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        assert_eq!(rotated.len(), 1);
        assert_eq!(
            std::fs::read(rotated[0].join("shard.bin")).unwrap(),
            b"data"
        );
        assert!(staging.is_dir());
        // Lock file holds a (numeric) pid; the log file exists.
        let lock = std::fs::read_to_string(flush_root.join(".active_pid")).unwrap();
        assert!(lock.parse::<i32>().is_ok());
        assert_eq!(std::fs::read_dir(&log_root).unwrap().count(), 1);
    }

    #[test]
    fn spawn_is_exclusive_while_lock_is_live() {
        let dir = tempfile::tempdir().unwrap();
        let (staging, flush_root, _) = layout(dir.path());
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("shard.bin"), b"data").unwrap();
        std::fs::create_dir_all(&flush_root).unwrap();
        // A live flush holds the lock (our own pid).
        std::fs::write(
            flush_root.join(".active_pid"),
            std::process::id().to_string(),
        )
        .unwrap();

        let started = spawn_fleet_flush_with("true", &staging, &mut |_| {}).unwrap();
        assert!(started);
        // Nothing rotated, staging untouched.
        assert_eq!(std::fs::read(staging.join("shard.bin")).unwrap(), b"data");
        assert!(std::fs::read_dir(&flush_root)
            .unwrap()
            .all(|e| !e.unwrap().path().is_dir()));
    }

    #[test]
    fn spawn_with_empty_staging_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (staging, flush_root, _) = layout(dir.path());
        std::fs::create_dir_all(&staging).unwrap();
        let started = spawn_fleet_flush_with("true", &staging, &mut |_| {}).unwrap();
        assert!(!started);
        assert!(!flush_root.join(".active_pid").exists());
    }
}
