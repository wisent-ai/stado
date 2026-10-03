//! The refusal this module states about itself, and the conversion for every
//! failure on this path that arrives as prose instead.

use crate::deploy::DeployError;

use super::allowlist::allowlist;

/// A host-exec failure that states its own [`crate::primitives::failure::FailureCode`]
/// where it is created, instead of leaving one to be guessed from its prose.
///
/// `Some(code)` preserves the classification known at the failure site.
/// `None` marks an upstream error that arrived only as prose and still needs
/// classification. Keep the approved-command help separate from the failure
/// message so command names and flags cannot change its error category.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExecRefusal {
    /// What this failure knows itself to be, when it knows.
    pub code: Option<crate::primitives::failure::FailureCode>,
    /// The operator sentence: what was refused, and why.
    pub message: String,
    /// Operator help that is not part of the failure — the approved
    /// spellings — printed beside it and never classified.
    pub help: Option<String>,
}

impl ExecRefusal {
    /// A refusal this module states outright: the words are understood, and
    /// the allowlist does not admit them.
    ///
    /// [`crate::primitives::failure::FailureCode::Refused`] — "an explicit policy refused
    /// this command" — is the whole of what happened. Nothing is missing, no
    /// credential was presented, nothing is down, and waiting changes
    /// nothing: only the words or the table can change. It is not retryable,
    /// and its exit code is the one the caller already chose.
    pub(super) fn unapproved(message: String) -> Self {
        Self {
            code: Some(crate::primitives::failure::FailureCode::Refused),
            message,
            help: Some(format!("approved commands: {}", allowlist())),
        }
    }
}

impl From<DeployError> for ExecRefusal {
    /// Everything else this module reaches — the registry, the channel, the
    /// host — still arrives as prose, and prose is what `classify_message`
    /// exists for.
    fn from(error: DeployError) -> Self {
        Self {
            code: None,
            message: error.0,
            help: None,
        }
    }
}
