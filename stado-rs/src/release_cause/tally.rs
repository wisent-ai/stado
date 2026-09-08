//! Counting causes across a host's quarantine history.

use super::cause::QuarantineCause;

/// How many quarantines share each cause, most common first.
///
/// Ties break on the cause's own word so two hosts with the same map print the
/// same order. [`QuarantineCause::Unclassified`] is counted like any other: it
/// is usually the largest bucket on a host with history, and hiding that would
/// misrepresent how much of the table is understood.
pub fn tally<I>(causes: I) -> Vec<(QuarantineCause, usize)>
where
    I: IntoIterator<Item = QuarantineCause>,
{
    let mut counts: std::collections::BTreeMap<QuarantineCause, usize> = Default::default();
    for cause in causes {
        *counts.entry(cause).or_default() += 1;
    }
    let mut tally: Vec<(QuarantineCause, usize)> = counts.into_iter().collect();
    tally.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });
    tally
}

/// The classified cause the most quarantines share, and how many.
///
/// `None` when nothing is classified. Deliberately not "the largest bucket":
/// an operator asking what dominates wants something they can act on, and
/// `unclassified` is not that — it is reported separately, as a count.
pub fn dominant(tally: &[(QuarantineCause, usize)]) -> Option<(QuarantineCause, usize)> {
    tally
        .iter()
        .find(|(cause, _)| cause.is_classified())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tally_ranks_by_count_and_names_a_dominant_cause() {
        let counts = tally([
            QuarantineCause::Unclassified,
            QuarantineCause::CredentialCannotServe,
            QuarantineCause::Unclassified,
            QuarantineCause::RollbackCompatibilityUndeclared,
            QuarantineCause::CredentialCannotServe,
            QuarantineCause::Unclassified,
        ]);
        assert_eq!(
            counts,
            vec![
                (QuarantineCause::Unclassified, 3),
                (QuarantineCause::CredentialCannotServe, 2),
                (QuarantineCause::RollbackCompatibilityUndeclared, 1),
            ]
        );
        // The largest bucket is unclassified; the answer an operator can act
        // on is the largest *classified* one.
        assert_eq!(
            dominant(&counts),
            Some((QuarantineCause::CredentialCannotServe, 2))
        );
        assert_eq!(dominant(&tally([QuarantineCause::Unclassified])), None);
    }
}
