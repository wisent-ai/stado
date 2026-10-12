//! The states a reconciliation records in its documents, each a type read
//! once from the document field that carries it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The root whose bytes win a conflict: the store that was primary before
/// the reconciliation, or its backup. Receipts, evidence and the served-store
/// payload carry it as a field; it is parsed into this type where it is read
/// and every decision matches the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictWinner {
    Primary,
    Backup,
}

impl ConflictWinner {
    /// The winner when the root that was primary before is (`true`) or is not
    /// the current primary.
    pub fn of_prior(prior_is_primary: bool) -> Self {
        if prior_is_primary {
            Self::Primary
        } else {
            Self::Backup
        }
    }

    /// The winner a document carries under `field`; a missing or unknown
    /// value is refused with the field's name and serde's own reason.
    pub fn read(document: &Value, field: &str) -> Result<Self, String> {
        let value = document
            .get(field)
            .ok_or_else(|| format!("{field} is missing"))?;
        Self::deserialize(value).map_err(|error| format!("{field}: {error}"))
    }
}

/// Where the durable lifecycle fence stands. The fence document carries it
/// as `status`; serde reads it once into this type and every decision
/// matches the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FenceStatus {
    Preparing,
    Fenced,
    Activating,
    Restoring,
    Activated,
    RollingBack,
    RolledBack,
}

impl FenceStatus {
    /// The status a document carries under `field`; a missing or unknown
    /// value is refused with the field's name and serde's own reason.
    pub fn read(document: &Value, field: &str) -> Result<Self, String> {
        let value = document
            .get(field)
            .ok_or_else(|| format!("{field} is missing"))?;
        Self::deserialize(value).map_err(|error| format!("{field}: {error}"))
    }
}

/// The word the fence document carries, from the same serde mapping.
impl std::fmt::Display for FenceStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}

/// Where the host's transaction receipt stands, in the order the program
/// writes it. The receipt carries it as `status`; it is read once into this
/// type and decisions match the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    /// No receipt exists for the transaction.
    Absent,
    Checkpointing,
    CheckpointReady,
    Applying,
    DataCommittedPendingActivation,
    ActivationEffectsArmed,
    RollbackEffectsArmed,
    ActivatedPendingLifecycle,
    Complete,
}

impl ReceiptStatus {
    /// The status a receipt carries under `status`; a missing or unknown
    /// value is refused with the field's name and serde's own reason.
    pub fn read(receipt: &Value) -> Result<Self, String> {
        let value = receipt
            .get("status")
            .ok_or_else(|| "receipt status is missing".to_string())?;
        Self::deserialize(value).map_err(|error| format!("receipt status: {error}"))
    }

    /// The checkpoint finished: any state past `checkpointing`.
    pub fn checkpointed(self) -> bool {
        !matches!(self, Self::Absent | Self::Checkpointing)
    }

    /// Data may still be applied or resumed: checkpointed, not yet committed
    /// past the frozen union.
    pub fn applicable(self) -> bool {
        matches!(
            self,
            Self::CheckpointReady | Self::Applying | Self::DataCommittedPendingActivation
        )
    }

    /// No activation effect has run yet.
    pub fn before_activation(self) -> bool {
        self.applicable()
            || matches!(
                self,
                Self::ActivationEffectsArmed | Self::RollbackEffectsArmed
            )
    }

    /// Before the data-commit boundary, where a rollback restores nothing it
    /// cannot.
    pub fn before_data_commit(self) -> bool {
        matches!(self, Self::CheckpointReady | Self::Applying)
    }

    /// Activated and waiting for, or past, lifecycle finalization.
    pub fn awaiting_finalization(self) -> bool {
        matches!(self, Self::ActivatedPendingLifecycle | Self::Complete)
    }
}

impl std::fmt::Display for ReceiptStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}

/// The actions a resident reconciliation worker carries out. The CLI worker
/// and the dashboard's control route both parse the requested action into
/// this type; anything else is refused with serde's own reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResidentAction {
    Run,
    Resume,
    Rollback,
    Finalize,
}

impl ResidentAction {
    pub fn parse(word: &str) -> Result<Self, String> {
        Self::deserialize(Value::String(word.to_string()))
            .map_err(|error| format!("resident worker action: {error}"))
    }
}

/// The serde word of a state, for refusals that name it.
fn word<T: Serialize>(state: &T, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match serde_json::to_value(state) {
        Ok(Value::String(word)) => formatter.write_str(&word),
        _ => Err(std::fmt::Error),
    }
}

/// Where one fenced writer unit stands, as the fence records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterStatus {
    /// Running as before; nothing done to it yet.
    Pending,
    StopIntent,
    Stopped,
    RestoreIntent,
    Restored,
}

/// Where a queue pause or restoration stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueEffectStatus {
    PauseIntent,
    RestoreIntent,
    Applied,
    /// Someone else changed the queue control first; theirs stands.
    Superseded,
}

/// Where a placement lease acquisition or the storage write fence stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldStatus {
    AcquireIntent,
    Acquired,
    ReleaseIntent,
    Released,
    /// Another owner holds the lease now.
    Superseded,
}

impl HoldStatus {
    /// Held by this transaction, or on the way to being released by it.
    pub fn held(self) -> bool {
        matches!(self, Self::Acquired | Self::ReleaseIntent)
    }

    /// Given up, by this transaction or to another owner.
    pub fn ended(self) -> bool {
        matches!(self, Self::Released | Self::Superseded)
    }
}

impl std::fmt::Display for WriterStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        word(self, formatter)
    }
}

impl std::fmt::Display for QueueEffectStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        word(self, formatter)
    }
}

impl std::fmt::Display for HoldStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        word(self, formatter)
    }
}
