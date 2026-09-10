//! The refusal this module states about itself, and the conversion for every
//! failure on this path that arrives as prose instead.

use crate::deploy::DeployError;

use super::allowlist::allowlist;

/// A host-exec failure that states its own [`crate::primitives::failure::FailureCode`]
/// where it is created, instead of leaving one to be guessed from its prose.
///
/// On 2026-09-03 `host exec charless-mac-mini -- ls -la …` was refused by the
/// allowlist and reported `error_code=timeout`, `retryable=true`. Nothing had
/// timed out. The refusal was built as a bare [`DeployError`], flattened to a
/// string by the CLI, and the code was then reconstructed by
/// [`crate::primitives::failure::classify_message`], whose `timeout` needle is the bare
/// substring `"timeout"` — and this refusal prints the whole allowlist, three
/// entries of which carry `--login-timeout-ms`. **The refusal matched its own
/// help text**, so every unapproved command on every host told its caller to
/// retry something that can never succeed.
///
/// Narrowing the needle would have left that design in place and handed the
/// next help-text collision to the next reader. So the code travels with the
/// failure: `code: Some(_)` is knowledge from the construction site and is
/// used verbatim, while `None` marks a failure that genuinely arrived as text
/// and keeps `classify_message` as its last resort.
///
/// `help` is the second half of the repair. The allowlist stays in front of
/// the operator, but out of `message`, so the classified and logged sentence
/// is the refusal itself — short enough to survive the log line's detail
/// bound whole, and with no vocabulary in it but its own.
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
    ///
    /// The code was added to `wisent-errors` for this call site rather than
    /// picked from the seven that were there. `not_found` reads as a missing
    /// path and would have sent an operator to check paths and permissions
    /// until they disbelieved the error, which is the cost the `timeout`
    /// misclassification was already imposing, only quieter.
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
