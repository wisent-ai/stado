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
use crate::providers::local::disk_cleanup::janitor::state::report::canonical::canonical_json;
use crate::providers::local::disk_cleanup::janitor::state::{
    read_state, reclaim_intent_digest, ControlUpdateAuthority,
};
use crate::providers::local::disk_cleanup::janitor::{
    STATE_LOCK_NAME, STATE_NAME, STATE_VERSION, WRITER_ATTEMPTS,
};
use crate::providers::local::disk_cleanup::{build_caches, safefs};

/// Python `_write_state`: lstat the destination (refuse symlink / foreign
/// owner), write to a sibling tempfile (O_EXCL, 0600), fsync, atomic
/// rename, fsync the directory.
pub(crate) fn write_state(
    state_dir: &Path,
    report: &Value,
    cursor: Option<&build_caches::BuildCachesCursor>,
    attempted_at: f64,
    control_update: ControlUpdateAuthority,
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
    // Merge while holding the state lock: a prevented writer may publish its
    // truthful observation concurrently with the run-lock owner, but must not
    // replace that owner's newer control checkpoint with the copy it read
    // before the owner finished.
    //
    // Every writer's stamp is carried forward and only this one is updated.
    // The interval gate reads the stamp belonging to the writer about to run,
    // so dropping the others here would restore cross-writer starvation.
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
    // is what `space report` reports and what `next_pass_at` is computed from -
    // and never moves backwards. An `interval_noop` anchors on its own older
    // stamp, and writing that verbatim would rewind a newer pass by another
    // writer.
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
    // purpose — but until this stamp existed nothing recorded it, so
    // `cleanup_success_age_seconds` downstream could not tell a prevented
    // janitor from a silent one and inferred a stall from the absence. On
    // 2026-09-03 charless-mac-mini ran one job for 42 minutes, roughly 40
    // in-process passes hit `lock_busy` at the ten-second agent tick, none of
    // them left a trace, and `host gates` turned `claiming` off on a host with
    // 17.3 GiB free, a 15 GiB watermark and `disk_pressure_unresolved: false`.
    //
    // A live workload takes only the kernel's shared hold; it does not write
    // the exclusive janitor holder record. Its expected answer is therefore
    // `lock_busy_unattributed`, not `lock_busy`. That unattributed answer is
    // known to be a legitimate prevention only when this agent also reports
    // one of its own slots live. Without that positive evidence it stays
    // unknown: a legacy or foreign holder must not make a silent janitor look
    // healthy indefinitely.
    //
    // Only the time is recorded here, not the holder: a `flock` owner cannot be
    // named from the process that failed to take it, and the one thing in this
    // product that can name it -- `space report`'s `cleanup_lock.holders` --
    // already does. What the arithmetic needs is prevented-since-a-known-time,
    // and that is what this is.
    let outcome = report.get("outcome").and_then(Value::as_str);
    let prevented_now = outcome == Some("lock_busy")
        || (outcome == Some("lock_busy_unattributed")
            && report
                .get("active_job_count")
                .and_then(Value::as_i64)
                .is_some_and(|count| count > 0));
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
    state.insert(WRITER_ATTEMPTS.to_string(), Value::Object(by_writer));
    state.insert("report".to_string(), report.clone());

    // Observation and control are deliberately separate. `report` remains the
    // last truthful event. Only the process admitted by the run lock may
    // replace or retire the policy-bound intent/checkpoint; every diagnostic
    // writer preserves the control state found inside this serialized merge.
    let outcome = report.get("outcome").and_then(Value::as_str);
    let policy_digest = report.get("policy_digest").and_then(Value::as_str);
    let scanned = report.get("cleaners").is_some_and(|value| !value.is_null());
    let enforce = report.get("mode").and_then(Value::as_str) == Some("enforce");
    let incomplete_scan = enforce
        && matches!(
            outcome,
            Some("cap_reached" | "partial_error" | "blocked_running_jobs")
        );
    let reached_target = match (
        report.get("free_bytes_after").and_then(Value::as_i64),
        report.get("target_bytes").and_then(Value::as_i64),
    ) {
        (Some(free), Some(target)) => free >= target,
        _ => false,
    };
    let previous_intent_digest = reclaim_intent_digest(&previous);
    let previous_intent = previous.get("reclaim_intent").cloned().or_else(|| {
        previous_intent_digest
            .map(|digest| serde_json::json!({ "policy_digest": digest, "outcome": "cap_reached" }))
    });
    let reclaim_intent = match control_update {
        ControlUpdateAuthority::Preserve => previous_intent,
        ControlUpdateAuthority::Owner if reached_target => None,
        ControlUpdateAuthority::Owner if scanned && incomplete_scan => policy_digest
            .map(|digest| serde_json::json!({ "policy_digest": digest, "outcome": outcome })),
        ControlUpdateAuthority::Owner if scanned => {
            // A real owner completed a scan under a resolved policy. That
            // either completed this intent or deliberately adopted a
            // different policy.
            None
        }
        ControlUpdateAuthority::Owner => previous_intent,
    };
    if let Some(intent) = reclaim_intent {
        state.insert("reclaim_intent".to_string(), intent);
    }

    let checkpoint = match control_update {
        ControlUpdateAuthority::Preserve => previous
            .get("build_caches_cursor")
            .cloned()
            .unwrap_or(Value::Null),
        ControlUpdateAuthority::Owner if scanned => {
            serde_json::to_value(cursor).map_err(|error| JanitorError::os(&error.to_string()))?
        }
        ControlUpdateAuthority::Owner => previous
            .get("build_caches_cursor")
            .cloned()
            .unwrap_or(Value::Null),
    };
    state.insert("build_caches_cursor".to_string(), checkpoint);
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
