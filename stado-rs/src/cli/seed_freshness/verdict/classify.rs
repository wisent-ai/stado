//! The join of the two halves into one of the eight outcomes.

use crate::cli::seed_freshness::verdict::inputs::{
    Attempt, SEED_DECLARED_EMPTY, SEED_FIELD_ABSENT, SEED_READ_UNSUPPORTED, SEED_UNREADABLE,
};
use crate::cli::seed_freshness::verdict::outcome::Verdict;

/// Decide one row's verdict from the vault's half and the run history's half.
///
/// `attempts` may arrive in any order; freshness is about the newest, so this
/// sorts rather than trusting a reader.
pub fn classify(seed_state: &str, attempts: &[Attempt]) -> Verdict {
    match seed_state {
        SEED_FIELD_ABSENT => return Verdict::FieldAbsent,
        SEED_DECLARED_EMPTY => return Verdict::FieldEmpty,
        SEED_UNREADABLE => return Verdict::VaultRowUnreadable,
        SEED_READ_UNSUPPORTED => return Verdict::VaultReadUnsupported,
        _ => {}
    }

    let mut ordered: Vec<&Attempt> = attempts.iter().collect();
    ordered.sort_by_key(|attempt| attempt.at_ms);

    let submitting: Vec<&&Attempt> = ordered
        .iter()
        .filter(|attempt| attempt.code_submitted)
        .collect();
    let Some(newest) = submitting.last() else {
        // Nothing ever put a code from this seed in front of the provider.
        // Failures that never reached the step are a different condition, and
        // no failures at all is simply untested.
        let unreached: Vec<&&Attempt> = ordered
            .iter()
            .filter(|attempt| attempt.authenticator_unreached || attempt.result != "signed_in")
            .collect();
        return if unreached.is_empty() {
            Verdict::PresentUntested
        } else {
            Verdict::PresentFailingElsewhere {
                attempts: unreached.len(),
            }
        };
    };

    // The run of refused submissions that ends the history, newest first. An
    // empty run means the newest submission matched: the last moment this
    // seed is known good. Otherwise the run is the streak that proves the seed
    // no longer matches, beginning at its oldest refusal.
    let streak: Vec<&&Attempt> = submitting
        .iter()
        .rev()
        .take_while(|attempt| attempt.code_rejected)
        .copied()
        .collect();
    match streak.last() {
        None => Verdict::LastKnownGood {
            at: newest.at.clone(),
        },
        Some(oldest_refusal) => Verdict::RejectedSince {
            since: oldest_refusal.at.clone(),
            attempts: streak.len(),
            locked_out: streak.iter().any(|attempt| attempt.locked_out),
        },
    }
}
