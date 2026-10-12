//! The closed vocabulary a finding is written in: how severe it is and how
//! sure the audit is, what it recommends, and which kind of resource it is
//! about. Each word is the variant's serde name, which is also what the
//! report, the plan and the human table print.

use serde::Serialize;
use serde_json::Value;

/// How severe a finding is, or how sure the audit is of it. Declaration
/// order is report order: high first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Level {
    High,
    Medium,
    Low,
}

/// What the audit recommends doing about a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Recommendation {
    Disable,
    Move,
    DisableOrMigrate,
    ReviewDeprovision,
    Delete,
    ReviewDelete,
    ReviewRelease,
}

impl Recommendation {
    /// The recommendation cannot be carried out by a plan: it changes the
    /// configuration or the provider fence, which a person decides.
    pub(super) fn blocked(self) -> bool {
        match self {
            Self::DisableOrMigrate | Self::ReviewDeprovision => true,
            Self::Disable
            | Self::Move
            | Self::Delete
            | Self::ReviewDelete
            | Self::ReviewRelease => false,
        }
    }
}

/// The kind of resource a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ResourceType {
    StorageBackup,
    Configuration,
    ProviderFence,
    AgentVm,
    PersistentDisk,
    StaticAddress,
    ManagedInstanceGroup,
    ComputeReservation,
}

fn serde_word<T: Serialize>(
    value: &T,
    formatter: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => formatter.write_str(&word),
        _ => Err(std::fmt::Error),
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

impl std::fmt::Display for Recommendation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}
