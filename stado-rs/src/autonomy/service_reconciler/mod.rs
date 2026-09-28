//! Reconcile registry-declared services against fresh host and endpoint facts.
//!
//! A beacon is the unit-side fact and a reachability sweep is the endpoint-side
//! fact. Neither is allowed to stand in for the other. Stale or unverified
//! evidence causes no mutation; a freshly missing unit with a freshly
//! unreachable endpoint may be recreated idempotently; a responding endpoint
//! is never duplicated merely because the beacon omitted its unit.
//!
//! [`endpoint`] folds the sweep into the endpoint-side fact, [`repair`] holds
//! one repair per kind of evidence that a declaration is wrong, [`receipts`]
//! holds the report and what is done with it, [`gate`] is the one shared
//! mutation gate, and [`run`] is the pass that puts a row through it.

mod endpoint;
mod gate;
mod predecessors;
mod receipts;
mod reconcilers;
mod repair;
mod run;

pub use receipts::{ServiceReconcileOutcome, ServiceReconcileReport, ServiceReconcileSummary};
pub use reconcilers::restore_reconcilers;
pub use run::reconcile;

pub(crate) const LATEST_REPORT: &str = "state/autonomy/services/latest.json";
const REPORT_PREFIX: &str = "state/autonomy/services/runs";
const SCHEMA_VERSION: u16 = 1;
