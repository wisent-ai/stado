//! Naming the cause behind one quarantine, deepest cause first.
//!
//! Split out of `classify/mod.rs`, which had grown past the module line cap;
//! the vocabulary, the envelope reader and the segmenting stay in their own
//! modules beside this one.

use super::envelope;
use super::needles::{
    CAPABILITY_REDEMPTION_NEEDLES, CAPABILITY_ROUTES_NEEDLES, CREDENTIAL_CANNOT_SERVE_NEEDLES,
    CREDENTIAL_STORE_NEEDLES, PROCESS_VANISHED_NEEDLES, READINESS_UNANSWERED_NEEDLES,
    ROLLBACK_COMPATIBILITY_NEEDLES, STABLE_BIND_OCCUPIED_NEEDLES,
};
use super::segments::{evidence_for, matches_any, strip_ansi};
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
}

/// Name the cause behind one quarantine, from the reason and whatever of the
/// candidate's own output is available.
///
/// Order encodes causality, not convenience, and it is the whole correctness
/// argument of this function. The recorded outage produced a log naming both
/// `no value at provider:kimi:…#value` and `capability is not issued`: the
/// empty vault field is why the capability was refused, so a classifier that
/// checked redemption first would have reported the consequence and sent the
/// operator to the capability lifecycle instead of the credential.
///
/// An envelope the emitting service wrote outranks all five: both of its
/// keys are declared vocabulary, so it says what broke without anyone
/// reading English, and its `detail` is the sentence the operator needs.
/// The sentences below remain for a line written before a product adopted
/// `wisent-errors`.
///
/// Otherwise the deepest thing the evidence can name wins:
///
/// 1. the agent's own refusal, which no log can contradict;
/// 2. the store not opening, below which nothing can work;
/// 3. a routed coordinate that cannot serve;
/// 4. no route at all;
/// 5. a capability refused when it was spent — last, because every cause above
///    can produce this sentence as a symptom.
///
/// Anything else is [`QuarantineCause::Unclassified`], with no evidence line:
/// there is no honest sentence to quote.
pub fn classify(text: &str) -> Classification {
    let clean = strip_ansi(text);
    if let Some(named) = envelope::classify(&clean) {
        return Classification {
            cause: named.cause,
            evidence: named.evidence,
        };
    }
    let haystack = clean.to_lowercase();
    for (needles, cause) in [
        (
            ROLLBACK_COMPATIBILITY_NEEDLES,
            QuarantineCause::RollbackCompatibilityUndeclared,
        ),
        (
            CREDENTIAL_STORE_NEEDLES,
            QuarantineCause::CredentialStoreUnreadable,
        ),
        (
            CREDENTIAL_CANNOT_SERVE_NEEDLES,
            QuarantineCause::CredentialCannotServe,
        ),
        (
            CAPABILITY_ROUTES_NEEDLES,
            QuarantineCause::CapabilityRoutesUnmapped,
        ),
        (
            CAPABILITY_REDEMPTION_NEEDLES,
            QuarantineCause::CapabilityRedemptionRefused,
        ),
        (
            STABLE_BIND_OCCUPIED_NEEDLES,
            QuarantineCause::StableBindOccupied,
        ),
        (
            READINESS_UNANSWERED_NEEDLES,
            QuarantineCause::ReadinessProbeUnanswered,
        ),
        (
            PROCESS_VANISHED_NEEDLES,
            QuarantineCause::ReleaseProcessVanished,
        ),
    ] {
        if matches_any(&haystack, needles) {
            return Classification {
                cause,
                evidence: evidence_for(&clean, needles),
            };
        }
    }
    Classification::unclassified()
}
