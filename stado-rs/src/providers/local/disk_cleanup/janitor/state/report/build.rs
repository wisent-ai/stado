//! Building the report: the base record and the timestamps it carries.

use std::time::SystemTime;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::{
    Caps, CleanerReport, CleanupReport,
};
use crate::providers::local::disk_cleanup::janitor::{MAX_ERRORS, STATE_VERSION};
use crate::providers::local::disk_cleanup::{
    backup_twins, build_caches, chromium_clones, queue_workdirs, release_store,
};
use crate::targets;

impl CleanupReport {
    pub fn base(active_job_count: i64, hostname: &str) -> Self {
        Self {
            hostname: targets::normalize_hostname(hostname),
            target_name: None,
            policy_digest: None,
            writer: "unknown",
            writer_version: crate::build_identity::BUILD_IDENTITY,
            policy_defaulted: false,
            mode: None,
            check_interval_seconds: None,
            started_at: utc_now(),
            duration_ms: 0,
            store_wait_ms: 0,
            outcome: "invalid_or_unavailable_policy".to_string(),
            free_bytes_before: None,
            free_bytes_after: None,
            low_bytes: None,
            target_bytes: None,
            pressure_active: None,
            hf: CleanerReport::default(),
            weles: CleanerReport::default(),
            builds: CleanerReport::default(),
            clones: CleanerReport::default(),
            workdirs: CleanerReport::default(),
            backup_twins: CleanerReport::default(),
            release_store: CleanerReport::default(),
            caps: Caps::default(),
            lock_busy: false,
            active_job_count: active_job_count.max(0),
            last_success_at: None,
            scanned: false,
            unscanned_cleaners: Vec::new(),
            unknown_cleaners: Vec::new(),
            builds_resume_from: None,
            builds_cursor: None,
            errors: Vec::new(),
        }
    }

    /// Python `_add_error`.
    pub fn add_error(&mut self, area: &str, exc: &JanitorError) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("{area}:{}", exc.error_code()));
        }
    }

    /// Python `_skip` for the HF cleaner.
    pub fn skip_hf(&mut self, reason: &str, count: i64) {
        *self.hf.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// Python `_skip` for the weles cleaner.
    pub fn skip_weles(&mut self, reason: &str, count: i64) {
        *self.weles.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// `_skip` for the build-cache cleaner (no Python original).
    pub fn skip_builds(&mut self, reason: &str, count: i64) {
        *self.builds.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// `_skip` for the Chromium clone cleaner (no Python original).
    pub fn skip_clones(&mut self, reason: &str, count: i64) {
        *self.clones.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    pub fn skip_workdirs(&mut self, reason: &str, count: i64) {
        *self.workdirs.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    pub fn skip_backup_twins(&mut self, reason: &str, count: i64) {
        *self
            .backup_twins
            .skipped
            .entry(reason.to_string())
            .or_insert(0) += count;
    }

    pub fn skip_release_store(&mut self, reason: &str, count: i64) {
        *self
            .release_store
            .skipped
            .entry(reason.to_string())
            .or_insert(0) += count;
    }

    /// The report as JSON (key order normalized at serialization sites
    /// with [`canonical_json`], matching Python `json.dumps(sort_keys=True)`).
    pub fn to_value(&self) -> Value {
        let cleaner = |c: &CleanerReport| {
            serde_json::json!({
                "scanned_items": c.scanned_items,
                "eligible_items": c.eligible_items,
                "deleted_items": c.deleted_items,
                "expected_bytes": c.expected_bytes,
                "actual_free_delta_bytes": c.actual_free_delta_bytes,
                "skipped": c.skipped,
            })
        };
        // A table of zeros and a table that was never filled in are the same
        // bytes, so a pass that did not reach its cleaners states the absence
        // instead. See [`CleanupReport::scanned`].
        let cleaners = if self.scanned {
            serde_json::json!({
                "huggingface_cache": cleaner(&self.hf),
                "weles_recordings": cleaner(&self.weles),
                "build_caches": cleaner(&self.builds),
                chromium_clones::CLEANER: cleaner(&self.clones),
                queue_workdirs::CLEANER: cleaner(&self.workdirs),
                backup_twins::CLEANER: cleaner(&self.backup_twins),
                release_store::CLEANER: cleaner(&self.release_store),
            })
        } else {
            Value::Null
        };
        serde_json::json!({
            "version": STATE_VERSION,
            "hostname": self.hostname,
            "target_name": self.target_name,
            "policy_digest": self.policy_digest,
            "writer": self.writer,
            "writer_version": self.writer_version,
            // The pid that wrote this pass. `writer` names WHICH entry point
            // ran and `writer_version` names what it was built from, and on
            // 2026-08-31 neither was enough: charless-mac-mini's state file
            // was overwritten every forty-five seconds by a build older than
            // the one that stamps those fields, so the file said nothing at
            // all about its author and no sampling caught the process alive.
            // A pid outlives the process in the file it wrote, which is the
            // whole difference between "somebody is writing this" and a name.
            "writer_pid": std::process::id(),
            "policy_defaulted": self.policy_defaulted,
            "mode": self.mode,
            "check_interval_seconds": self.check_interval_seconds,
            "started_at": self.started_at,
            "duration_ms": self.duration_ms,
            "store_wait_ms": self.store_wait_ms,
            "outcome": self.outcome,
            "free_bytes_before": self.free_bytes_before,
            "free_bytes_after": self.free_bytes_after,
            "low_bytes": self.low_bytes,
            "target_bytes": self.target_bytes,
            "pressure_active": self.pressure_active,
            "cleaners": cleaners,
            "unscanned_cleaners": self.unscanned_cleaners,
            "unknown_cleaners": self.unknown_cleaners,
            "caps": {
                "bytes": self.caps.bytes,
                "items": self.caps.items,
                "scan": self.caps.scan,
                "deadline": self.caps.deadline,
            },
            "lock_busy": self.lock_busy,
            "active_job_count": self.active_job_count,
            "last_success_at": self.last_success_at,
            "build_caches_resume_from": self.builds_resume_from,
            "build_caches_pending_directories": self.builds_cursor.as_ref().map_or(0, build_caches::BuildCachesCursor::pending_directories),
            "errors": self.errors,
        })
    }
}

/// Python `_utc_now` (`datetime.now(timezone.utc).isoformat()`).
pub(crate) fn utc_now() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Python `time.time()` as f64 seconds.
pub(crate) fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
