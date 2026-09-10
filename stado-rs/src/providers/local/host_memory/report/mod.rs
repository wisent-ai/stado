//! The memory pass's report, its outcome vocabulary, and the one reader the
//! capacity publisher uses.
//!
//! The outcome names are the disk janitor's, not a parallel set. An operator
//! who has learned that `healthy_noop` means "the watermark was not crossed"
//! and `lock_busy` means "another writer had the pass" already knows what
//! this pass is telling them, and `space report` prints both passes side by
//! side.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::reading::MemoryReading;
use super::state;

mod placement;
pub use placement::{placement_decision, PlacementDecision};

/// No pass has ever completed on this host.
pub const NEVER_RUN: &str = "never_run";
/// The canonical registry could not be read, or what it declares is not a
/// policy this binary can execute.
pub const INVALID_OR_UNAVAILABLE_POLICY: &str = "invalid_or_unavailable_policy";
/// Another writer holds the pass lock.
pub const LOCK_BUSY: &str = "lock_busy";
/// This writer ran inside its own declared interval.
pub const INTERVAL_NOOP: &str = "interval_noop";
/// The host is under both watermarks; there was nothing to do.
pub const HEALTHY_NOOP: &str = "healthy_noop";
/// The host is over a watermark and the mode forbids repairing it.
pub const REPORT_ONLY: &str = "report_only";
/// The host is over a watermark and jobs are running that a repair would
/// disturb.
pub const BLOCKED_RUNNING_JOBS: &str = "blocked_running_jobs";
/// Repairs ran and the host reached its declared target.
pub const RECLAIMED_TARGET: &str = "reclaimed_target";
/// Repairs ran, memory was returned, the target is not reached yet.
pub const RECLAIMED_PROGRESS: &str = "reclaimed_progress";
/// The per-pass repair budget stopped the pass.
pub const CAP_REACHED: &str = "cap_reached";
/// A repair was attempted and failed; the rest of the pass still ran.
pub const PARTIAL_ERROR: &str = "partial_error";
/// The host is over a watermark and no declared repair matched anything.
pub const NO_ELIGIBLE_ITEMS: &str = "no_eligible_items";

/// What one declared repair did on one pass.
#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    /// Subjects the repair examined.
    pub examined: i64,
    /// Subjects the repair would act on.
    pub eligible: i64,
    /// Subjects the repair acted on.
    pub repaired: i64,
    /// Why a subject was not acted on, counted by reason.
    pub skipped: BTreeMap<String, i64>,
    /// The declared subjects this repair named, so a `report` pass says what
    /// an `enforce` pass would touch.
    pub subjects: Vec<String>,
}

impl RepairReport {
    fn to_value(&self) -> Value {
        json!({
            "examined": self.examined,
            "eligible": self.eligible,
            "repaired": self.repaired,
            "skipped": self.skipped,
            "subjects": self.subjects,
        })
    }
}

/// Which budget stopped a pass.
#[derive(Debug, Clone, Default)]
pub struct MemoryCaps {
    pub repairs: bool,
    pub deadline: bool,
}

impl MemoryCaps {
    pub fn any(&self) -> bool {
        self.repairs || self.deadline
    }
}

/// One pass's whole answer.
#[derive(Debug, Clone)]
pub struct MemoryReport {
    pub hostname: String,
    pub target_name: Option<String>,
    pub policy_digest: Option<String>,
    pub writer: &'static str,
    pub writer_version: &'static str,
    /// True when this host declares no `memory_reclaim` and the reporting
    /// default is in force. `mode: report` alone cannot tell a deliberate
    /// choice from an absent declaration.
    pub policy_defaulted: bool,
    pub mode: Option<String>,
    pub check_interval_seconds: Option<i64>,
    pub started_at: String,
    pub duration_ms: i64,
    pub outcome: String,
    pub before: MemoryReading,
    pub after: Option<MemoryReading>,
    pub low_bytes: Option<i64>,
    pub target_bytes: Option<i64>,
    pub high_swap_used_pct: Option<i64>,
    pub pressure_active: Option<bool>,
    pub refuse_placement: bool,
    /// The exact admission reason a capacity publication will carry while
    /// this reading stands, or `None` when this host's declaration does not
    /// refuse work. Computed by [`refusal_reason`], which is the same
    /// function [`placement_refusal`] answers the publisher with, so the
    /// report and the publication can never disagree about whether the host
    /// is refusing.
    pub placement_refusal: Option<&'static str>,
    pub repairs: BTreeMap<String, RepairReport>,
    /// Whether this pass reached its repairs at all. A pass that stopped at
    /// the interval gate emits `repairs: null` rather than a table of zeros,
    /// which is byte-for-byte what a pass that looked and found nothing
    /// emits — the distinction the disk janitor paid fifteen days to learn.
    pub examined_repairs: bool,
    pub caps: MemoryCaps,
    pub lock_busy: bool,
    pub active_job_count: i64,
    pub last_success_at: Option<String>,
    pub errors: Vec<String>,
}

fn reading_value(reading: &MemoryReading) -> Value {
    json!({
        "available_bytes": reading.available_bytes,
        "available_mb": reading.available_mb(),
        "total_bytes": reading.total_bytes,
        "swap_used_bytes": reading.swap_used_bytes,
        "swap_total_bytes": reading.swap_total_bytes,
        "swap_used_pct": reading.swap_used_pct(),
        "compressor_pages": reading.compressor_pages,
        "swapouts": reading.swapouts,
    })
}

impl MemoryReport {
    /// The report as the state file and every reader see it.
    pub fn to_value(&self) -> Value {
        let repairs = if self.examined_repairs {
            Value::Object(
                self.repairs
                    .iter()
                    .map(|(name, report)| (name.clone(), report.to_value()))
                    .collect::<Map<String, Value>>(),
            )
        } else {
            Value::Null
        };
        json!({
            "hostname": self.hostname,
            "target_name": self.target_name,
            "policy_digest": self.policy_digest,
            "writer": self.writer,
            "writer_version": self.writer_version,
            "policy_defaulted": self.policy_defaulted,
            "mode": self.mode,
            "check_interval_seconds": self.check_interval_seconds,
            "started_at": self.started_at,
            "duration_ms": self.duration_ms,
            "outcome": self.outcome,
            "memory_before": reading_value(&self.before),
            "memory_after": self.after.as_ref().map(reading_value),
            "low_bytes": self.low_bytes,
            "target_bytes": self.target_bytes,
            "high_swap_used_pct": self.high_swap_used_pct,
            "pressure_active": self.pressure_active,
            "refuse_placement": self.refuse_placement,
            "placement_refusal": self.placement_refusal,
            "repairs": repairs,
            "caps": {"repairs": self.caps.repairs, "deadline": self.caps.deadline},
            "lock_busy": self.lock_busy,
            "active_job_count": self.active_job_count,
            "last_success_at": self.last_success_at,
            "errors": self.errors,
        })
    }
}

/// The admission reason a declaration produces for one watermark verdict.
///
/// One function, two callers: the pass writes its answer into the report so
/// an operator can read the refusal, and [`placement_refusal`] answers the
/// capacity publisher with it. A second predicate would eventually let the
/// report say "refused" while the publication said `accepting_jobs: true`.
pub fn refusal_reason(
    refuse_placement: bool,
    over_watermark: Option<bool>,
) -> Option<&'static str> {
    if refuse_placement && over_watermark == Some(true) {
        Some(MEMORY_PRESSURE_ACTIVE)
    } else {
        None
    }
}

/// The watermark a host published for itself, read from the state the last
/// completed pass wrote.
///
/// This is the disk path's idiom, and for the disk path's reason: the
/// publisher must not read the canonical registry on every tick, and the
/// janitor already resolved the declaration the host is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedWatermark {
    pub low_bytes: i64,
    pub high_swap_used_pct: i64,
    pub refuse_placement: bool,
}

/// The watermark this host last recorded for itself, if any.
pub fn persisted_watermark_in(home: &std::path::Path) -> Option<PublishedWatermark> {
    let state = state::read_state_in(home);
    if state.get("version").and_then(Value::as_i64) != Some(super::constants::STATE_VERSION) {
        return None;
    }
    let report = state.get("report")?;
    Some(PublishedWatermark {
        low_bytes: report.get("low_bytes").and_then(Value::as_i64)?,
        high_swap_used_pct: report.get("high_swap_used_pct").and_then(Value::as_i64)?,
        refuse_placement: report
            .get("refuse_placement")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The watermark this host last recorded for itself.
pub fn persisted_watermark() -> Option<PublishedWatermark> {
    persisted_watermark_in(&crate::config_file::expand_tilde("~"))
}

/// Whether a live reading is over a published watermark.
pub fn over_published_watermark(
    watermark: PublishedWatermark,
    reading: &MemoryReading,
) -> Option<bool> {
    let by_memory = reading
        .available_bytes
        .map(|available| available < watermark.low_bytes);
    let by_swap = reading
        .swap_used_pct()
        .map(|pct| pct >= watermark.high_swap_used_pct);
    match (by_memory, by_swap) {
        (None, None) => None,
        (memory, swap) => Some(memory.unwrap_or(false) || swap.unwrap_or(false)),
    }
}

/// The admission reason a capacity publication must carry, or `None` when
/// this host's own declaration does not refuse work.
///
/// Read at publication time by [`crate::queue::capacity::publish_capacity`],
/// which is the one place both writers of a capacity document pass through.
pub fn placement_refusal() -> Option<&'static str> {
    let watermark = persisted_watermark()?;
    let reading = super::reading::read_host_memory();
    refusal_reason(
        watermark.refuse_placement,
        over_published_watermark(watermark, &reading),
    )
}

/// The admission reason published while a host is over its memory watermark.
pub const MEMORY_PRESSURE_ACTIVE: &str = "memory_pressure_active";

/// The last report this host wrote, for readers that only want the outcome.
pub fn last_report_in(home: &std::path::Path) -> Value {
    state::read_state_in(home)
        .get("report")
        .cloned()
        .unwrap_or(Value::Null)
}
