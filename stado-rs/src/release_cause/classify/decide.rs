//! Naming the cause behind one quarantine from what carries it as structure:
//! the failure envelope a product wrote, or the agent's own observation.
//!
//! What the agent observes itself — a release refused for rollback
//! compatibility, a pid gone, a port held by another process, a probe that
//! timed out — it names where it observes it ([`crate::release_agent`]), and
//! a product's own refusal is read from its `wisent-errors` envelope
//! ([`super::envelope`]). A log line without an envelope names no cause.

use super::envelope;
use super::segments::{evidence_line, strip_ansi};
use crate::release_cause::cause::QuarantineCause;

/// The cause a quarantine's evidence names, and the exact line it was read
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub cause: QuarantineCause,
    /// The one line the cause was taken from, ANSI-stripped and bounded.
    /// Empty exactly when the cause is [`QuarantineCause::Unclassified`].
    pub evidence: String,
}

impl Classification {
    fn unclassified() -> Self {
        Self {
            cause: QuarantineCause::Unclassified,
            evidence: String::new(),
        }
    }

    /// What the agent itself observed, with the sentence it observed it in.
    pub fn observed(cause: QuarantineCause, sentence: &str) -> Self {
        if !cause.is_classified() {
            return Self::unclassified();
        }
        Self {
            cause,
            evidence: evidence_line(&strip_ansi(sentence)),
        }
    }
}

/// The cause the failure envelopes in `text` name, or
/// [`QuarantineCause::Unclassified`] when none names one this rollout knows.
///
/// A product's envelope is the deepest evidence there is: it is why the
/// candidate stopped, so it outranks whatever the agent saw from outside.
pub fn classify(text: &str) -> Classification {
    match envelope::classify(&strip_ansi(text)) {
        Some(named) => Classification {
            cause: named.cause,
            evidence: named.evidence,
        },
        None => Classification::unclassified(),
    }
}

/// The product's own envelope in `logs` when it names a cause, otherwise what
/// the agent observed.
pub fn classify_observed(logs: &str, observed: Classification) -> Classification {
    let named = classify(logs);
    if named.cause.is_classified() {
        named
    } else {
        observed
    }
}
