//! Workspace ownership is a held OS lock, not the presence of a receipt.
use crate::common::{atomic_json, Runtime};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
struct Record {
    workspace: PathBuf,
    pid: u32,
    parent_pid: libc::pid_t,
    started_at: DateTime<Utc>,
}

pub(super) struct Instance {
    pub workspace: PathBuf,
    _instance_lock: File,
    _lock: File,
}

fn directory(runtime: &Runtime) -> PathBuf {
    runtime.home.join("editor/instances")
}

impl Instance {
    pub fn acquire(runtime: &Runtime, workspace: &Path, parent_pid: libc::pid_t) -> Result<Self> {
        let directory = directory(runtime);
        fs::create_dir_all(&directory)
            .with_context(|| format!("create editor instance directory {}", directory.display()))?;
        let key = hex::encode(Sha256::digest(workspace.as_os_str().as_encoded_bytes()));
        let path = directory.join(key).with_extension("lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open editor workspace lock {}", path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                bail!("editor workspace {} already has an instance holding {}; inspect stado product editor status", workspace.display(), path.display());
            }
            Err(std::fs::TryLockError::Error(cause)) => {
                return Err(cause)
                    .with_context(|| format!("acquire editor workspace lock {}", path.display()))
            }
        }
        // A new connection gets a new live identity. A crashed predecessor's
        // receipt cannot become live again when its workspace lock is reused.
        let live_path = directory
            .join(uuid::Uuid::new_v4().to_string())
            .with_extension("instance");
        let instance_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&live_path)
            .with_context(|| format!("create editor instance lock {}", live_path.display()))?;
        match instance_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                bail!(
                    "new editor instance lock {} is already held",
                    live_path.display()
                );
            }
            Err(std::fs::TryLockError::Error(cause)) => {
                return Err(cause).with_context(|| {
                    format!("acquire editor instance lock {}", live_path.display())
                });
            }
        }
        let receipt = live_path.with_extension("json");
        let record = Record {
            workspace: workspace.to_path_buf(),
            pid: std::process::id(),
            parent_pid,
            started_at: Utc::now(),
        };
        atomic_json(&receipt, &serde_json::to_value(record)?)?;
        Ok(Self {
            workspace: workspace.to_path_buf(),
            _lock: lock,
            _instance_lock: instance_lock,
        })
    }
}

pub(super) fn status(runtime: &Runtime, workspace: Option<&Path>) -> Result<Value> {
    let directory = directory(runtime);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => Some(entries),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
        Err(cause) => {
            return Err(cause)
                .with_context(|| format!("list editor instances in {}", directory.display()))
        }
    };
    let observed_at = Utc::now();
    let mut instances = Vec::new();
    for entry in entries.into_iter().flatten() {
        let path = entry
            .context("read editor instance directory entry")?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("instance") {
            continue;
        }
        let lock = File::open(&path)
            .with_context(|| format!("open editor instance lock {}", path.display()))?;
        match lock.try_lock_shared() {
            Ok(()) => continue,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(cause)) => {
                return Err(cause)
                    .with_context(|| format!("observe editor instance lock {}", path.display()))
            }
        }
        let receipt = path.with_extension("json");
        let record: Record =
            serde_json::from_slice(&fs::read(&receipt).with_context(|| {
                format!("read held editor instance receipt {}", receipt.display())
            })?)
            .with_context(|| {
                format!("decode held editor instance receipt {}", receipt.display())
            })?;
        if workspace.is_some_and(|workspace| workspace != record.workspace) {
            continue;
        }
        let age_seconds = observed_at
            .signed_duration_since(record.started_at)
            .num_seconds();
        instances.push(json!({"workspace": record.workspace, "pid": record.pid,
            "parent_pid": record.parent_pid, "started_at": record.started_at,
            "age_seconds": age_seconds, "state": "running", "lock": path}));
    }
    instances.sort_by(|left, right| left["workspace"].as_str().cmp(&right["workspace"].as_str()));
    Ok(json!({"observed_at": observed_at, "instances": instances}))
}
