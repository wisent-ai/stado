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

/// What kind of failure a repair met, stated where it is met so the row's
/// classification never has to be read back out of the sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::autonomy::service_reconciler) enum FailureKind {
    /// The unit is there but nothing proves which process owns it; refused
    /// before any host command ran.
    IdentityUnresolved,
    /// The declaration does not say what the unit runs; refused before any
    /// host command ran.
    DeclarationIncomplete,
    /// A host mutation was attempted and failed.
    RepairFailed,
}

impl FailureKind {
    pub(in crate::autonomy::service_reconciler) fn classification(self) -> &'static str {
        match self {
            Self::IdentityUnresolved => "identity_unresolved",
            Self::DeclarationIncomplete => "declaration_incomplete",
            Self::RepairFailed => "repair_failed",
        }
    }
}

/// A repair that did not complete: its kind and the operator's sentence.
#[derive(Debug, Clone)]
pub(in crate::autonomy::service_reconciler) struct RepairRefused {
    pub kind: FailureKind,
    pub detail: String,
}

impl RepairRefused {
    pub(in crate::autonomy::service_reconciler) fn new(kind: FailureKind, detail: String) -> Self {
        Self { kind, detail }
    }
}

/// A failure whose kind nobody stated is a failed mutation.
impl From<String> for RepairRefused {
    fn from(detail: String) -> Self {
        Self::new(FailureKind::RepairFailed, detail)
    }
}
