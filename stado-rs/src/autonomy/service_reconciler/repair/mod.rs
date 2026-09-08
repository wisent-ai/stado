//! One repair per kind of evidence that a declared unit is wrong.
//!
//! [`plan`] renders the unit a repair would assert and writes the corrected
//! declaration; every repair here goes through it, so no path can install a
//! different program than an operator's `ensure` for the same name.
//! [`observed`] holds the two endpoint-evidenced repairs, [`beacon`] the one
//! repair allowed to run on unknown evidence, and [`undeclared`] the repair
//! that takes the host channel as its evidence because no endpoint exists to
//! disprove.

mod beacon;
mod observed;
mod plan;
mod undeclared;

pub(in crate::autonomy::service_reconciler) use beacon::reconcile_beacon;
pub(in crate::autonomy::service_reconciler) use observed::{
    reconcile_observed, reconcile_unreachable,
};
pub(in crate::autonomy::service_reconciler) use undeclared::reconcile_undeclared;
