//! What a standby pass answers: the words it ends with, its report and the
//! receipt of each unit it staged.

use serde::Serialize;

use super::Delivery;

/// The words a standby pass ends with.
pub mod words {
    /// A release-controlled tree is declared for the host but its agent has
    /// not rolled it out yet; the units that depend on it were not staged.
    pub const AWAITING_RELEASE_ROLLOUT: &str = "awaiting_release_rollout";
    /// Every unit is staged and the host is written into the profile and
    /// the directory.
    pub const DECLARED: &str = "declared";
}

#[derive(Debug, Clone, Serialize)]
pub struct StandbyReport {
    pub profile: String,
    pub host: String,
    pub placed_on: String,
    pub platform: String,
    /// Why the operator asked this host to stand by.
    pub reason: String,
    pub outcome: String,
    pub deliveries: Vec<Delivery>,
    pub units: Vec<UnitReceipt>,
    pub registry_generation: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitReceipt {
    pub service: String,
    pub label: String,
    pub action: String,
    /// The unit file staged on the host.
    pub path: String,
}
