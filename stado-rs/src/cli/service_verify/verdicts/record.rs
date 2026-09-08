//! Keeping what was seen, so a later question does not start from zero.

use crate::observations::{service_fact, Observation};

use crate::cli::service_verify::Finding;

/// Write down what was seen, where a later question can find it.
///
/// A probe that only prints has verified nothing five minutes from now: the
/// sweep runs, the table scrolls past, and the next component to ask "is this
/// declaration true" starts from zero and takes the declaration's own word for
/// it -- which is the position the fleet was in for twelve days. The record is
/// what lets an answer outlive the process that obtained it, and it is the
/// file [`crate::observations::freshness`] reads to decide whether an answer
/// is old enough to need asking again.
///
/// One record per finding that looked, `unverified` ones included. "Nobody
/// could look" is a fact about the fleet worth keeping: it is the difference
/// between a service nobody has checked since Tuesday and one checked a
/// minute ago.
///
/// A standby row is not recorded, because it is not an observation. Nothing
/// looked at it and nothing ever will while the service is elsewhere, so it
/// does not decay and has no age worth storing. It would also collide: the
/// record is keyed by `(fact, vantage)`, and a standby host that is also
/// handed a dial address produces two rows under one key -- whichever landed
/// last would decide whether the fleet remembers `observed` or `unverified`
/// for a probe that did happen.
///
/// A failed write is reported and never fatal. The rows on screen are true
/// regardless, and a full disk must not turn a working verifier into a command
/// that exits non-zero for a reason no service caused.
pub(in crate::cli::service_verify) fn record_observations(findings: &[Finding]) {
    let observations: Vec<Observation> = findings
        .iter()
        .filter(|finding| finding.probed)
        .map(|finding| {
            Observation::now(
                service_fact(&finding.service, &finding.host),
                finding.host.clone(),
                finding.state,
                finding.detail.clone(),
            )
        })
        .collect();
    if let Err(error) = crate::observations::record(&observations) {
        eprintln!("could not record what was observed: {error}");
    }
}
