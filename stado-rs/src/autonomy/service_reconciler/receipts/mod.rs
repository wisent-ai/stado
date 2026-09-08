//! What a reconciliation run leaves behind.
//!
//! [`report`] holds the document itself, [`persist`] stores it under both the
//! immutable run key and the latest pointer, and [`transitions`] announces a
//! refusal once, on the run where it changed.

mod persist;
mod report;
mod transitions;

pub(in crate::autonomy::service_reconciler) use persist::persist_report;
pub(in crate::autonomy::service_reconciler) use transitions::alert_transitions;

pub use report::{ServiceReconcileOutcome, ServiceReconcileReport, ServiceReconcileSummary};
