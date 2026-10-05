//! The atomic state-file write transaction.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::SystemTime;

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::open_lock_at;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::promise::{
    pass_was_prevented, promises_after, PROMISES,
};
use crate::providers::local::disk_cleanup::janitor::state::read_state;
use crate::providers::local::disk_cleanup::janitor::state::report::canonical::canonical_json;
use crate::providers::local::disk_cleanup::janitor::{
    STATE_LOCK_NAME, STATE_NAME, STATE_VERSION, WRITER_ATTEMPTS,
};
use crate::providers::local::disk_cleanup::safefs;

/// Python `_write_state`: lstat the destination (refuse symlink / foreign
/// owner), write to a sibling tempfile (O_EXCL, 0600), fsync, atomic
/// rename, fsync the directory.
pub(crate) fn write_state(
    state_dir: &Path,
    report: &Value,
    attempted_at: f64,
) -> Result<(), JanitorError> {
    let state_lock = open_lock_at(&state_dir.join(STATE_LOCK_NAME))?;
    fs2::FileExt::lock_exclusive(&state_lock)?;
    let destination = state_dir.join(STATE_NAME);
    match std::fs::symlink_metadata(&destination) {
        Ok(existing) => {
            if existing.file_type().is_symlink() || !existing.is_file() || existing.uid() != euid()
            {
                return Err(JanitorError::os("unsafe cleanup state"));
            }
        }
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => {}
        Err(exc) => return Err(exc.into()),
    }
    // Merge while holding the state lock, so two writers finishing together
    // do not drop each other's stamps. Every writer's stamp is carried
    // forward and only this one is updated.
    let previous = read_state(state_dir).unwrap_or_else(|_| Value::Object(Map::new()));
    let mut by_writer = previous
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(writer) = report.get("writer").and_then(Value::as_str) {
        by_writer.insert(writer.to_string(), serde_json::json!(attempted_at));
    }
    // `last_attempt_at` keeps its meaning - the last attempt by ANYONE, which
    // is what `space report` reports - and never moves backwards: a slower
    // writer finishing after a newer pass by another writer must not rewind
    // it.
    let last_attempt_at = previous
        .get("last_attempt_at")
        .and_then(Value::as_f64)
        .map_or(attempted_at, |recorded| recorded.max(attempted_at));
    // When the last pass was prevented rather than run, and never moving
    // backwards either.
    //
    // A janitor that cannot take the run lock because a workload holds it in
    // shared mode has been PREVENTED. That is a modelled, healthy answer —
    // `acquire_workload_lock` takes the lock for the job's whole duration on
    // purpose — but without this stamp nothing recorded it, so
    // `cleanup_success_age_seconds` downstream could not tell a prevented
    // janitor from a silent one and inferred a stall from the absence: one
    // long job, dozens of in-process passes hitting `lock_busy` without a
    // trace, and `host gates` turning `claiming` off on a host with free
    // space above its watermark and `disk_pressure_unresolved: false`.
    //
    // A live workload takes only the kernel's shared hold; see
    // [`pass_was_prevented`] for which answers count as prevention.
    //
    // Only the time is recorded here, not the holder: a `flock` owner cannot be
    // named from the process that failed to take it, and the one thing in this
    // product that can name it -- `space report`'s `cleanup_lock.holders` --
    // already does. What the arithmetic needs is prevented-since-a-known-time,
    // and that is what this is.
    let prevented_now = pass_was_prevented(report);
    let last_prevented_at = previous
        .get("last_prevented_at")
        .and_then(Value::as_f64)
        .map_or_else(
            || prevented_now.then_some(attempted_at),
            |recorded| {
                Some(if prevented_now {
                    recorded.max(attempted_at)
                } else {
                    recorded
                })
            },
        );
    // Success belongs to the host, not to the observing pass. Since a busy
    // writer can finish after the owner whose report it copied, retain the
    // later valid RFC 3339 timestamp rather than trusting write order.
    let mut report = report.clone();
    let incoming_success = report
        .get("last_success_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    let recorded_success = previous
        .get("report")
        .and_then(|previous| previous.get("last_success_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let parsed =
        |stamp: &str| chrono::DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok();
    let last_success_at = match (incoming_success, recorded_success) {
        (Some(incoming), Some(recorded)) => match (parsed(&incoming), parsed(&recorded)) {
            (Some(incoming_at), Some(recorded_at)) if recorded_at > incoming_at => Some(recorded),
            (None, Some(_)) => Some(recorded),
            _ => Some(incoming),
        },
        (incoming, recorded) => incoming.or(recorded),
    };
    if let (Some(object), Some(stamp)) = (report.as_object_mut(), last_success_at) {
        object.insert("last_success_at".to_string(), Value::String(stamp));
    }
    let mut state = Map::new();
    state.insert("version".to_string(), serde_json::json!(STATE_VERSION));
    state.insert(
        "last_attempt_at".to_string(),
        serde_json::json!(last_attempt_at),
    );
    if let Some(stamp) = last_prevented_at {
        state.insert("last_prevented_at".to_string(), serde_json::json!(stamp));
    }
    state.insert(
        PROMISES.to_string(),
        Value::Object(promises_after(&previous, &report, attempted_at)),
    );
    state.insert(WRITER_ATTEMPTS.to_string(), Value::Object(by_writer));
    state.insert("report".to_string(), report.clone());
    let payload = canonical_json(&Value::Object(state));
    // Tempfile uniqueness like Python's f".{name}.{getpid()}.{monotonic_ns()}".
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = state_dir.join(format!(".{STATE_NAME}.{}.{nanos}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(payload.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &destination)?;
        let dir_fd = safefs::open_dir_path(state_dir)?;
        safefs::fsync(dir_fd.as_raw_fd())?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&temp);
    result
}
