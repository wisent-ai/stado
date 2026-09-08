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
    if submitting.is_empty() {
        // Nothing ever put a code from this seed in front of the provider.
        // Failures that never reached the step are a different condition, and
        // no failures at all is simply untested.
        let unreached = ordered
            .iter()
            .filter(|attempt| attempt.authenticator_unreached || attempt.result != "signed_in")
            .count();
        return if unreached > 0 {
            Verdict::PresentFailingElsewhere {
                attempts: unreached,
            }
        } else {
            Verdict::PresentUntested
        };
    }

    // The newest submission that was NOT refused is the last moment this seed
    // is known to have matched. Everything after it, if every one of those was
    // refused, is the streak that proves the seed no longer matches.
    let last_good = submitting
        .iter()
        .rposition(|attempt| !attempt.code_rejected);
    match last_good {
        Some(index) if index + 1 == submitting.len() => Verdict::LastKnownGood {
            at: submitting[index].at.clone(),
        },
        other => {
            let start = other.map_or(0, |index| index + 1);
            let streak = &submitting[start..];
            Verdict::RejectedSince {
                since: streak
                    .first()
                    .map(|attempt| attempt.at.clone())
                    .unwrap_or_default(),
                attempts: streak.len(),
                locked_out: streak.iter().any(|attempt| attempt.locked_out),
            }
        }
    }
}
