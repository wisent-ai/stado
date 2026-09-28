//! Launch the resident storage-root worker, exactly once per transaction.
//!
//! Under the launch lock: a worker the native manager already runs (or is
//! starting) is acknowledged, not replaced. Otherwise the operation lock is
//! taken and the manager asked again, since a unit may start between the
//! two; only then is the launch intent recorded, the staged tool verified
//! and moved into place, and the unit installed and started. The launcher
//! then waits for the worker to record itself as the owner bound to that
//! manager process.

mod manager;
mod origin;
mod unit;

use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::expand_home;
use manager::{acknowledge_running, manager_bound_owner, manager_state, running};

const LOCK_FILE_MODE: u32 = 0o600;
const TOOL_MODE: u32 = 0o700;
const PRIVATE_DIRECTORY: u32 = 0o700;

pub(super) struct Request<'a> {
    pub transaction: &'a str,
    pub staged: &'a str,
    pub tool: &'a str,
    pub sha256: &'a str,
    pub arguments: &'a str,
}

/// One launch, with every path it touches.
struct Launch<'a> {
    transaction: &'a str,
    label: String,
    work: String,
    owner_path: String,
    intent_path: String,
    log_path: String,
    argv: Vec<String>,
    action: String,
}

fn open_lock(path: &str) -> Result<fs::File, String> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(LOCK_FILE_MODE)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("cannot open {path}: {error}"))
}

fn flock(file: &fs::File, operation: i32) -> std::io::Result<()> {
    // SAFETY: the descriptor belongs to `file`, open for the whole call.
    if unsafe { nix::libc::flock(file.as_raw_fd(), operation) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn atomic_json(path: &str, value: &Value) -> Result<(), String> {
    let staged = format!("{path}.{}.new", std::process::id());
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(LOCK_FILE_MODE)
            .open(&staged)?;
        serde_json::to_writer(&mut file, value)?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        fs::rename(&staged, path)?;
        fs::File::open(Path::new(path).parent().unwrap_or(Path::new("/")))?.sync_all()
    })();
    written.map_err(|error| format!("cannot write {path}: {error}"))
}

fn option<'v>(arguments: &'v [String], name: &str) -> Result<&'v str, String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
        .ok_or_else(|| format!("native worker arguments omit {name}"))
}

/// Verify the staged tool's digest and move it to the tool's own path.
fn place_tool(staged: &str, tool: &str, expected: &str, work: &str) -> Result<(), String> {
    let info = fs::symlink_metadata(staged)
        .map_err(|error| format!("cannot inspect {staged}: {error}"))?;
    if !info.file_type().is_file() {
        return Err("staged transaction tool is not a regular file".to_string());
    }
    let bytes = fs::read(staged).map_err(|error| format!("cannot read {staged}: {error}"))?;
    if hex::encode(Sha256::digest(&bytes)) != expected {
        return Err("staged transaction tool digest mismatch".to_string());
    }
    fs::set_permissions(staged, fs::Permissions::from_mode(TOOL_MODE))
        .and_then(|()| fs::rename(staged, tool))
        .and_then(|()| fs::File::open(work)?.sync_all())
        .map_err(|error| format!("cannot place the transaction tool: {error}"))
}

pub(super) fn launch(request: &Request) -> Result<(), String> {
    let tool = expand_home(request.tool)?;
    let staged = expand_home(request.staged)?;
    let work = Path::new(&tool)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .ok_or_else(|| "transaction tool has no directory".to_string())?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIRECTORY)
        .create(&work)
        .map_err(|error| format!("cannot create {work}: {error}"))?;
    let arguments: Vec<String> = base64::engine::general_purpose::STANDARD
        .decode(request.arguments)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("native worker arguments are invalid: {error}"))?;
    let captured_target: Value = base64::engine::general_purpose::STANDARD
        .decode(option(&arguments, "--target-config")?)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("captured target is invalid: {error}"))?;
    let launch = Launch {
        transaction: request.transaction,
        label: format!(
            "com.wisent.stado-storage-root-reconcile.{}",
            request.transaction
        ),
        owner_path: format!("{work}/operation-owner.json"),
        intent_path: format!("{work}/launch-intent.json"),
        log_path: format!("{work}/transaction-worker.log"),
        argv: std::iter::once(tool.clone())
            .chain(arguments.iter().cloned())
            .collect(),
        action: option(&arguments, "--phase")?.to_string(),
        work,
    };
    let recovery = Path::new(&launch.work)
        .parent()
        .and_then(Path::parent)
        .map(|recovery| recovery.to_string_lossy().into_owned())
        .ok_or_else(|| "transaction directory has no recovery root".to_string())?;
    let launch_lock = open_lock(&format!("{recovery}/storage-root-reconcile.launch.lock"))?;
    flock(&launch_lock, nix::libc::LOCK_EX)
        .map_err(|error| format!("cannot take the launch lock: {error}"))?;
    let state = manager_state(&launch)?;
    if running(&state) {
        return acknowledge_running(&launch, &state);
    }
    let operation_lock = open_lock(&format!("{recovery}/storage-root-reconcile.lock"))?;
    if flock(&operation_lock, nix::libc::LOCK_EX | nix::libc::LOCK_NB).is_err() {
        let state = manager_state(&launch)?;
        if running(&state) {
            return acknowledge_running(&launch, &state);
        }
        return Err("native reconciliation lock is held without a manager-bound owner".to_string());
    }
    // Manager visibility and the operation lock are one observation: a unit
    // that started after the first read but before the lock was won still
    // forbids replacement.
    let state = manager_state(&launch)?;
    if running(&state) {
        drop(operation_lock);
        return acknowledge_running(&launch, &state);
    }
    let lock_info = operation_lock
        .metadata()
        .map_err(|error| format!("cannot inspect the operation lock: {error}"))?;
    let release_api = origin::captured_release_api(&launch, &captured_target)?;
    let mut intent = json!({
        "schema": "stado.storage-root-launch.v1",
        "transaction": launch.transaction,
        "target": captured_target.get("name"),
        "target_config": captured_target,
        "action": launch.action,
        "status": "launch_intent",
        "source_revision": option(&arguments, "--source-revision")?,
        "tool_sha256": request.sha256,
        "release_api": release_api,
        "native_manager": state,
        "lock_device": lock_info.dev(),
        "lock_inode": lock_info.ino(),
    });
    atomic_json(&launch.intent_path, &intent)?;
    place_tool(&staged, &tool, request.sha256, &launch.work)?;
    unit::install(&launch, &release_api, move || drop(operation_lock))?;
    loop {
        let state = manager_state(&launch)?;
        if let Some(owner) = manager_bound_owner(&launch, &state)? {
            intent["status"] = json!("worker_adopted");
            intent["native_manager"] = state;
            atomic_json(&launch.intent_path, &intent)?;
            println!("STADO_RECONCILE_OWNER\t{owner}");
            return Ok(());
        }
        if !running(&state) {
            return Err(
                "native reconciliation worker did not record manager-bound ownership".to_string(),
            );
        }
    }
}
