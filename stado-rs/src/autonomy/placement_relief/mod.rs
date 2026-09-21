//! Move a placement profile off a host that is over its memory watermark.
//!
//! A host's memory policy repairs the host from the inside: it ends session
//! processes, restarts the units it was told it may restart, and publishes
//! `memory_pressure_active` while it is still over its watermark. Nothing
//! above it ever read that word and acted across the fleet. On 2026-09-18 the
//! 16 GiB control host carried Brama, its entitlements router, Skarbiec, a
//! Weles browser and the fleet's object store; it sat at 1.4 GiB available
//! with swap at 71%, its janitor reported progress every pass, its object
//! store closed connections on every release write, and a laptop declared in
//! the same placement profile with four times the memory did nothing, because
//! `stado placement move` waited for an operator to type it.
//!
//! This stage is that operator. Each autonomy tick it reads every placement
//! profile, the host it is placed on and the memory every declared host
//! publishes, and when the placed host is over its watermark and another
//! declared host has more headroom it runs the same transaction
//! `stado placement move` runs — same claim, same execution, same rollback —
//! under the same rails as every other autonomous mutation: report mode
//! plans and records, the emergency pause and circuit breaker block, one
//! lease per profile, one relocation per tick, and a profile moved within
//! the cooldown is left where it is so two hosts cannot hand a profile back
//! and forth.
//!
//! [`evidence`] reads what each host says about its memory, [`plan`] decides
//! what every profile needs from that evidence alone, and [`run`] puts each
//! planned move through the shared mutation gate. `stado placement relief`
//! prints the plan without the gate, so an operator can read what the next
//! tick would do.

mod evidence;
mod plan;
mod run;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use evidence::{host_memory, HostMemory};
pub use plan::{plan, Candidate, DueAction, ReliefOutcome};
pub use run::reconcile;

pub(crate) const LATEST_REPORT: &str = "state/autonomy/placement_relief/latest.json";
const REPORT_PREFIX: &str = "state/autonomy/placement_relief/runs";
const SCHEMA_VERSION: u16 = 1;

/// How long after a relocation a profile stays where it landed, whatever the
/// evidence says. A move stops every unit in the profile, copies its state
/// and starts it elsewhere; the source host's next memory reading has to see
/// those processes gone and the destination's has to see them warm before
/// either reading means anything. The host memory janitor passes at most
/// every five minutes and publishes after each pass; thirty minutes is six
/// publications on both sides, enough for a swapped-out source to page its
/// remaining processes back in and for a destination that could not carry
/// the load to say so.
pub const RELOCATION_COOLDOWN_SECONDS: i64 = 1800;

/// How long one published pressure reading keeps a host pressured for this
/// stage, whatever its next publication says.
///
/// The decision used to be one instantaneous sample. charless-mac-mini
/// declares a 2 GiB floor and oscillates across it every few minutes: on
/// 2026-09-21 the tick at 18:01:19Z read `2.5 GiB available, pressure clear`
/// and settled the profile, while `stado placement relief` typed seconds
/// later read `1.7 GiB available, pressure active` — and every hand reading
/// that hour saw pressure. A host in that state is not healthy between the
/// dips; it is a host with no memory left, and a stage that samples it once
/// per tick relieves it only by luck. Pressure therefore sticks for this
/// window, and a host has to publish clear for the whole of it before the
/// profile on it settles. Three times the memory pass's five-minute cadence,
/// so a genuinely recovered host is settled within a quarter of an hour.
pub const PRESSURE_STICKY_SECONDS: i64 = 900;

/// Relocations one tick may execute. One: every destination's headroom was
/// measured before the first move, and a second profile placed onto the same
/// host in the same tick would be placed on headroom the first move already
/// spent.
pub const MAX_RELOCATIONS_PER_TICK: usize = 1;

/// The words a row is classified with. Written once so the report, the CLI
/// and the tests read the same vocabulary.
pub mod words {
    /// The placed host is under its watermark; nothing to do.
    pub const SETTLED: &str = "settled";
    /// The placed host's memory publication is missing or stale; nothing
    /// mutates on evidence nobody has refreshed.
    pub const EVIDENCE_STALE: &str = "evidence_stale";
    /// The placed host is over its watermark and no other declared host has
    /// more headroom; the row names every candidate and why it was refused.
    pub const NO_DESTINATION: &str = "no_destination_with_headroom";
    /// The profile was relocated within the cooldown.
    pub const MOVED_RECENTLY: &str = "moved_recently";
    /// The profile cannot be moved by anyone: split, incomplete or
    /// release-controlled.
    pub const PROFILE_UNMOVABLE: &str = "profile_unmovable";
    /// A move is due, and report mode or the emergency pause recorded it
    /// without executing.
    pub const PLANNED: &str = "planned";
    /// A move is due, but this host is not the directory authority; only the
    /// authority commits a placement transaction.
    pub const AUTHORITY_ELSEWHERE: &str = "authority_elsewhere";
    /// A move is due, but this tick already spent its relocation.
    pub const ACTION_LIMIT: &str = "action_limit";
    /// A move is due, but the pause or the circuit breaker became active.
    pub const CONTROL_BLOCKED: &str = "control_blocked";
    /// A move is due, but another reconciler holds the profile's lease.
    pub const LEASE_BLOCKED: &str = "lease_blocked";
    /// The profile was moved.
    pub const RELOCATED: &str = "relocated";
    /// The move ran and failed; the row carries the failure and its rollback.
    pub const RELOCATION_FAILED: &str = "relocation_failed";
    /// No declared host had headroom; a registered host was prepared to
    /// stand by, and the next tick may move there.
    pub const STANDBY_PREPARED: &str = "standby_prepared";
    /// The standby pass declared deliveries and stopped at a rollout the
    /// host's own agent still has to stage; a later tick continues.
    pub const STANDBY_PENDING: &str = "standby_pending";
    /// The standby pass was refused; the row carries the refusal.
    pub const STANDBY_REFUSED: &str = "standby_refused";
}

/// One profile's row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReliefRow {
    pub profile: String,
    pub services: Vec<String>,
    pub placed_on: Option<String>,
    pub classification: String,
    pub destination: Option<String>,
    pub candidates: Vec<Candidate>,
    pub detail: String,
    /// The transaction a relocation committed, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReliefSummary {
    pub profiles: usize,
    pub pressured: usize,
    pub planned: usize,
    pub relocated: usize,
    pub blocked: usize,
    pub failures: usize,
}

/// What one pass left behind, and when each profile last landed somewhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReliefReport {
    pub schema_version: u16,
    pub decision_id: String,
    pub created_at: String,
    pub mode: crate::autonomy::policy::AutonomyMode,
    pub summary: ReliefSummary,
    pub rows: Vec<ReliefRow>,
    /// Profile name to the RFC 3339 instant of its last relocation. Carried
    /// forward from the previous report, so a cooldown survives ticks that
    /// move nothing.
    #[serde(default)]
    pub relocations: BTreeMap<String, String>,
    /// Host name to the RFC 3339 instant it last published memory pressure.
    /// Carried forward, so a host that dips below its watermark between two
    /// ticks is still treated as pressured by the next one.
    #[serde(default)]
    pub pressure_seen: BTreeMap<String, String>,
}
