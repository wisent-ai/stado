//! Canonical object layout and atomic persistence for the autonomy control plane.
//!
//! The components are the seams this file already carried: `objects` reads and
//! writes one object by type and answers "which ids exist, and how old are
//! they" from a single listing, `control` holds the emergency pause and the
//! mutation circuit breaker, `leases` holds the placement lease and its
//! acquire, renew and release transitions, and `records` holds one component
//! per record kind — the policy, the inventory snapshot, the decisions, and
//! the feedback, savings and adoption ledger. The object layout itself stays
//! here, because every component is named for one prefix declared below.
//! Every name a caller outside this module uses is re-exported here, so
//! `crate::autonomy::storage::<item>` resolves exactly as before.

mod control;
mod leases;
mod objects;
mod records;

pub use control::{load_control, record_mutation_outcome, set_control, ControlState};
pub use leases::{
    acquire_placement_lease, release_placement_lease, release_placement_lease_exact,
    renew_placement_lease, PlacementLease,
};
pub use objects::{read_json, write_json};
pub use records::decisions::{
    list_decision_index, list_decisions, load_decision, update_decision, write_decision,
};
pub use records::inventory::{load_latest_inventory, publish_inventory};
pub use records::ledger::adoptions::{list_adoptions, write_adoption};
pub use records::ledger::feedback::{
    list_feedback_decision_ids, list_recent_feedback, write_feedback, PlacementFeedback,
};
pub use records::ledger::savings::{
    list_measured_savings_ids, list_savings, list_savings_ids, list_savings_measurements,
    load_savings, write_savings, write_savings_measurement,
};
pub use records::policy::{load_policy, load_policy_versioned, write_policy};

/// Root every autonomy object shares, and the reason it is not `autonomy/`.
///
/// The object gateway authorizes a write by matching its key against this
/// deployment's namespace prefix allowlist. `autonomy/` is in no namespace's
/// list, so every write under it came back
/// `401 {"error":"unauthorized or non-immutable release write"}` — a sentence
/// naming neither the namespace, the prefix, nor the grant. Reads kept working
/// from the `local` backup backend, so `optimize status` printed a forecast
/// while the whole layer had been unable to persist since 2026-08-19.
///
/// `state/` is authorized, and this is the same move
/// [`crate::monitor::host_silence::SILENCE_PREFIX`] already made after the same
/// 401 for the same reason.
pub const OBJECT_ROOT: &str = "state/autonomy";

/// One autonomy object path, rooted under [`OBJECT_ROOT`] at compile time.
macro_rules! autonomy_object {
    ($suffix:literal) => {
        concat!("state/autonomy/", $suffix)
    };
}

pub const POLICY_PATH: &str = autonomy_object!("policy.json");
pub const INVENTORY_LATEST_PATH: &str = autonomy_object!("inventory/latest.json");
pub const CONTROL_PATH: &str = autonomy_object!("control.json");
const INVENTORY_PREFIX: &str = autonomy_object!("inventory/snapshots");
const DECISION_PREFIX: &str = autonomy_object!("decisions");
const LEASE_PREFIX: &str = autonomy_object!("leases");
const SAVINGS_MEASUREMENT_PREFIX: &str = autonomy_object!("savings-measurements");
const SAVINGS_PREFIX: &str = autonomy_object!("savings");
const ADOPTION_PREFIX: &str = autonomy_object!("adoptions");
const FEEDBACK_PREFIX: &str = autonomy_object!("feedback");
