//! Addresses declared for a move that has not happened: listed, never dialled.

use crate::observations::UNVERIFIED;
use crate::targets::ServiceDirectory;

use crate::cli::service_verify::finding::STANDBY_DETAIL;
use crate::cli::service_verify::Finding;

/// Every standby address the directory declares, as a row of its own.
///
/// Nothing is dialled here, and that is the point. A standby address is where
/// a host would serve if the service moved to it, so while the service is
/// elsewhere nothing is listening and silence is the declared state. Probing
/// it would file `unreachable` against a fleet working exactly as declared --
/// which is what happened to `brama` on a standby laptop, back when one field
/// carried both meanings.
///
/// Listed rather than dropped, because an address nobody prints is an address
/// nobody maintains until the move that needs it, and the wrong port is then
/// discovered during a cutover. `unverified` is the honest word for a row
/// nobody looked at, and it is the one state
/// [`fail_on_unreachable`](crate::cli::service_verify::verdicts::fail_on_unreachable) never
/// counts.
///
/// Built from the directory rather than gathered per host: a standby address
/// has no vantage it could be checked from, it is the same string on every
/// machine, and a host holding nothing but a standby address is never visited
/// by the sweep at all -- so collecting these per host would drop exactly the
/// rows only this function can produce.
pub(in crate::cli::service_verify) fn standby_findings(
    directory: &ServiceDirectory,
    only: Option<&str>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, service) in &directory.services {
        for (host, endpoint) in &service.standby {
            if only.is_some_and(|wanted| wanted != host.as_str()) {
                continue;
            }
            findings.push(Finding {
                service: name.clone(),
                host: host.clone(),
                endpoint: endpoint.url.clone(),
                state: UNVERIFIED,
                detail: STANDBY_DETAIL.to_string(),
                probed: false,
            });
        }
    }
    findings
}
