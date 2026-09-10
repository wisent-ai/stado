//! The shape one host's claiming verdict is carried in.

use super::read::DiagnosticRead;
use std::collections::BTreeMap;

/// Everything one host answered about whether it can claim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostGates {
    /// The registry target name, not the operator's spelling of it.
    pub host: String,
    /// `blockers.is_empty()`. A host currently using all measured resources is
    /// busy, not broken; `accepting_jobs` reports that transient state without
    /// turning it into an operational blocker.
    pub claiming: bool,
    pub blockers: Vec<String>,
    pub disk_pressure_unresolved: bool,
    /// [`DISK_CLEANUP_STALLED`]: the janitor has not completed a pass within
    /// `STALL_INTERVALS` of its own declared interval. Carried as a field and
    /// not only as a blocker string because the release verdict embeds it
    /// beside `disk_pressure_unresolved` ([`gates_section`]), and an operator
    /// reading "free 45 GiB against a 100 GiB watermark" has to be able to see
    /// in the same object whether anything is still trying to fix it.
    ///
    /// [`DISK_CLEANUP_STALLED`]: super::DISK_CLEANUP_STALLED
    /// [`gates_section`]: super::gates_section
    pub disk_cleanup_stalled: bool,
    /// [`DISK_CLEANUP_LOCK_HELD`]: the janitor is being refused the run lock
    /// AND has completed nothing inside the stall window. Carried beside
    /// `disk_cleanup_stalled` and never merged into it: the two are mutually
    /// exclusive by construction and name different remedies.
    ///
    /// [`DISK_CLEANUP_LOCK_HELD`]: super::DISK_CLEANUP_LOCK_HELD
    pub disk_cleanup_lock_held: bool,
    /// Seconds since a pass was last PREVENTED from taking the run lock, or
    /// `None` when none was. The number behind `disk_cleanup_lock_held`, and
    /// the one that distinguishes "a workload is holding it right now" from "a
    /// hold has outlived its workload".
    pub cleanup_prevented_age_seconds: Option<i64>,
    /// Seconds since the janitor last completed a pass, or `None` when it has
    /// never recorded one. `None` with a declared interval is the fifteen-day
    /// case, and is not the same finding as "it succeeded a long time ago".
    pub cleanup_success_age_seconds: Option<i64>,
    /// Available bytes from the host's `df -Pk /` reading, never from a pressure flag.
    pub free_bytes: Option<u64>,
    /// The same available space as GiB, rounded to one decimal.
    pub free_gb: Option<f64>,
    /// The threshold admission is actually gated on: the janitor's own
    /// validated watermark first, the registry declaration second — the same
    /// order the agent resolves it in.
    pub low_watermark_gb: Option<i64>,
    pub target_free_gb: Option<i64>,
    pub policy_mode: Option<String>,
    pub published_at: Option<String>,
    pub age_seconds: Option<i64>,
    /// The worker's current admission decision and the measured resources that
    /// explain it. `None` means this host published no corresponding value.
    pub accepting_jobs: Option<bool>,
    pub running_jobs: Option<i64>,
    pub available_cpu_cores: Option<i64>,
    pub total_cpu_cores: Option<i64>,
    pub available_accelerators: BTreeMap<String, i64>,
    pub free_ram_gb: Option<f64>,
    pub total_ram_gb: Option<f64>,
    pub free_vram_gb: Option<i64>,
    pub total_vram_gb: Option<i64>,
    /// What this host published about its own memory: the declaration's
    /// refusal, the reading behind it and both watermarks. The disk half of
    /// this struct has always been complete; a host refusing every job for
    /// memory pressure reported two RAM totals and a flag nothing read.
    pub memory: super::verdict::MemoryGate,
    /// The storage backend this host's own installed binary resolves from the
    /// config its services consume, or `None` when the host would not answer
    /// with one ([`AGENT_STORE_UNREADABLE`]). Reported beside
    /// `fleet_store_backend` and never compared away to a boolean: an
    /// operator has to be able to read "that host writes to `local`, the fleet
    /// reads `stado`" off one screen and go fix the unit that set it.
    ///
    /// [`AGENT_STORE_UNREADABLE`]: super::AGENT_STORE_UNREADABLE
    pub agent_store_backend: Option<String>,
    /// The storage backend THIS control plane reads
    /// ([`crate::config::wc_storage_backend`]) — the other half of the
    /// sentence, because a backend name alone says nothing about whether the
    /// two ends agree.
    pub fleet_store_backend: String,
    /// Findings that are true and are NOT reasons this host claims nothing, so
    /// they can never change `claiming` or the exit status: an operator needs
    /// them to act, and a note that could fail a script would be suppressed
    /// within the week.
    pub notes: Vec<String>,
    /// How many local APFS snapshots the host is holding, or `None` where the
    /// host could not be asked (every Linux host).
    pub local_snapshots: Option<usize>,
    /// Queued jobs pinned to this host, oldest first. This is the gate's
    /// consequence made visible: a host that claims nothing while work is
    /// pinned to it is starving that exact list, and "blocked" without the
    /// starved work named is a verdict nobody can size.
    pub waiting_jobs: Vec<WaitingJob>,
    /// Incomplete reads are not a claiming or disk-full verdict.
    pub complete: bool,
    pub observations: Vec<DiagnosticRead>,
    pub pressure_source: Option<&'static str>,
    pub published_diagnostics: Option<serde_json::Value>,
}

/// One queued job a non-claiming host is starving.
#[derive(Debug, Clone, PartialEq)]
pub struct WaitingJob {
    pub job_id: String,
    pub age_seconds: Option<i64>,
}
