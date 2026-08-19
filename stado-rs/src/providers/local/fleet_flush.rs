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

/// Rotate a staging dir and flush it through an explicitly configured
/// external adapter. Both the Python interpreter and staging directory are
/// opt-in; the base Stado agent has no Python or Hugging Face dependency.
pub async fn spawn_fleet_flush(
    fleet_staging: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> std::io::Result<bool> {
    let token = crate::skarbiec::read_string("stado-huggingface", "write_token")
        .await
        .map_err(std::io::Error::other)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            std::io::Error::other("Skarbiec item stado-huggingface field write_token is required")
        })?;
    let python = std::env::var("STADO_HF_FLUSH_PYTHON").map_err(|_| {
        std::io::Error::other(
            "STADO_HF_FLUSH_PYTHON is required when Hugging Face flush is enabled",
        )
    })?;
    spawn_fleet_flush_with_token(&python, fleet_staging, log_fn, &token)
}

/// Explicit-interpreter seam retained for deterministic callers.
pub fn spawn_fleet_flush_with(
    python: &str,
    fleet_staging: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> std::io::Result<bool> {
    spawn_fleet_flush_with_token(python, fleet_staging, log_fn, "")
}

fn spawn_fleet_flush_with_token(
    python: &str,
    fleet_staging: &Path,
    log_fn: &mut dyn FnMut(&str),
    token: &str,
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
    let mut command = std::process::Command::new(python);
    command
        .args(["-c", FLUSH_RUNNER])
        .arg(&flush_dir)
        .arg(&lock_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(err_file))
        .process_group(i32::default());
    if !token.is_empty() {
        command
            .env("HF_TOKEN", token)
            .env("HUGGING_FACE_HUB_TOKEN", token);
    }
    let child = command.spawn()?;
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

