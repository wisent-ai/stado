//! The reconciliation report document, its per-service rows and its tally.

use serde::{Deserialize, Serialize};

use crate::autonomy::policy::AutonomyMode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileSummary {
    pub services: usize,
    pub missing: usize,
    pub unknown: usize,
    pub planned: usize,
    pub changed: usize,
    pub blocked: usize,
    pub failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileOutcome {
    pub host: String,
    pub service: String,
    pub unit: String,
    pub beacon_state: String,
    pub endpoint_state: String,
    pub classification: String,
    pub action: String,
    pub changed: bool,
    pub detail: String,
}

impl ServiceReconcileOutcome {
    pub(super) fn key(&self) -> String {
        format!("{}:{}", self.host, self.service)
    }

    pub(super) fn needs_alert(&self) -> bool {
        matches!(
            self.classification.as_str(),
            "repair_failed"
                | "identity_unresolved"
                | "declaration_incomplete"
                | "endpoint_unverified"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileReport {
    pub schema_version: u16,
    pub created_at: String,
    pub mode: AutonomyMode,
    pub summary: ServiceReconcileSummary,
    pub outcomes: Vec<ServiceReconcileOutcome>,
}
