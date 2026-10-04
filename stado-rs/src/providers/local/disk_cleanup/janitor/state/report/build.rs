//! Building the report: the base record and the timestamps it carries.

use std::time::SystemTime;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::{CleanerReport, CleanupReport};
use crate::providers::local::disk_cleanup::janitor::STATE_VERSION;
use crate::providers::local::disk_cleanup::rule::{self, VolumeReading};
use crate::providers::local::disk_cleanup::{
    agent_logs, backup_twins, chromium_clones, job_outputs, local_snapshots, object_evidence,
    queue_workdirs, release_store,
};
use crate::targets;

impl CleanupReport {
    pub fn base(active_job_count: i64, hostname: &str) -> Self {
        Self {
            hostname: targets::normalize_hostname(hostname),
            target_name: None,
            writer: "unknown",
            writer_version: crate::binary::build_identity::BUILD_IDENTITY,
            started_at: utc_now(),
            duration_ms: 0,
            store_wait_ms: 0,
            outcome: "volume_unreadable".to_string(),
            free_bytes_before: None,
            free_bytes_after: None,
            total_bytes: None,
            used_percent_before: None,
            used_percent_after: None,
            pressure_active: None,
            hf: CleanerReport::default(),
            weles: CleanerReport::default(),
            builds: CleanerReport::default(),
            clones: CleanerReport::default(),
            workdirs: CleanerReport::default(),
            job_outputs: CleanerReport::default(),
            backup_twins: CleanerReport::default(),
            release_store: CleanerReport::default(),
            local_snapshots: CleanerReport::default(),
            object_evidence: CleanerReport::default(),
            agent_logs: CleanerReport::default(),
            lock_busy: false,
            active_job_count: active_job_count.max(0),
            last_success_at: None,
            scanned: false,
            errors: Vec::new(),
        }
    }

    /// Record the volume reading a pass is judged on.
    pub fn record_reading(&mut self, reading: VolumeReading) {
        self.total_bytes = Some(reading.total_bytes);
        self.free_bytes_before = Some(reading.free_bytes);
        self.free_bytes_after = Some(reading.free_bytes);
        self.used_percent_before = Some(reading.used_percent());
        self.used_percent_after = Some(reading.used_percent());
        self.pressure_active = Some(reading.full());
    }

    /// The reading the pass began with, when it took one.
    pub fn reading_before(&self) -> Option<VolumeReading> {
        Some(VolumeReading {
            total_bytes: self.total_bytes?,
            free_bytes: self.free_bytes_before?,
        })
    }

    /// Python `_add_error`.
    pub fn add_error(&mut self, area: &str, exc: &JanitorError) {
        self.errors
            .push(format!("{area}:{} ({exc})", exc.error_code()));
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

    pub fn skip_job_outputs(&mut self, reason: &str, count: i64) {
        *self
            .job_outputs
            .skipped
            .entry(reason.to_string())
            .or_insert(0) += count;
    }

    /// The same skip, with the bytes it left behind.
    pub fn keep_job_outputs(&mut self, reason: &str, bytes: i64) {
        self.skip_job_outputs(reason, 1);
        *self
            .job_outputs
            .skipped_bytes
            .entry(reason.to_string())
            .or_insert(0) += bytes;
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
                "skipped_bytes": c.skipped_bytes,
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
                job_outputs::CLEANER: cleaner(&self.job_outputs),
                backup_twins::CLEANER: cleaner(&self.backup_twins),
                release_store::CLEANER: cleaner(&self.release_store),
                local_snapshots::CLEANER: cleaner(&self.local_snapshots),
                object_evidence::CLEANER: cleaner(&self.object_evidence),
                agent_logs::CLEANER: cleaner(&self.agent_logs),
            })
        } else {
            Value::Null
        };
        serde_json::json!({
            "version": STATE_VERSION,
            "hostname": self.hostname,
            "target_name": self.target_name,
            "writer": self.writer,
            "writer_version": self.writer_version,
            // The pid that wrote this pass. `writer` names WHICH entry point
            // ran and `writer_version` names what it was built from, and
            // neither is enough when a build older than the one that stamps
            // those fields overwrites the file every few seconds: the file
            // then says nothing about its author and no sampling catches the
            // process alive.
            // A pid outlives the process in the file it wrote, which is the
            // whole difference between "somebody is writing this" and a name.
            "writer_pid": std::process::id(),
            "rule": rule::rule_json(self.reading_before()),
            "started_at": self.started_at,
            "duration_ms": self.duration_ms,
            "store_wait_ms": self.store_wait_ms,
            "outcome": self.outcome,
            "total_bytes": self.total_bytes,
            "free_bytes_before": self.free_bytes_before,
            "free_bytes_after": self.free_bytes_after,
            "used_percent_before": self.used_percent_before,
            "used_percent_after": self.used_percent_after,
            "pressure_active": self.pressure_active,
            "cleaners": cleaners,
            "lock_busy": self.lock_busy,
            "active_job_count": self.active_job_count,
            "last_success_at": self.last_success_at,
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
