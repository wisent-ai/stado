//! Addresses declared for a move that has not happened: listed everywhere,
//! and dialled only from the standby host itself, where an answer means a
//! second copy is serving.

use crate::observations::{OBSERVED, STANDBY_SERVING, UNVERIFIED};
use crate::targets::ServiceDirectory;

use crate::cli::service_verify::finding::STANDBY_DETAIL;
use crate::cli::service_verify::probe::probe;
use crate::cli::service_verify::Finding;

/// Every standby address the directory declares, as a row of its own.
///
/// Nothing is dialled here. A standby address is where a host would serve if
/// the service moved to it, so while the service is elsewhere nothing is
/// listening and silence is the declared state. Probing it from another host
/// would file `unreachable` against a fleet working exactly as declared --
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
/// is the same string on every machine, and a host holding nothing but a
/// standby address would otherwise drop exactly the rows only this function
/// can produce. [`serving_standbys`] is the one look that is taken.
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

/// The standby addresses THIS host holds that answer, probed from this host.
///
/// Silence is the declared state and yields no row; [`standby_findings`]
/// already lists the address. An answer is the failure the listing alone
/// hides: a standby that serves keeps its own copy of the service's state, so
/// what the active host writes never reaches it and the two copies diverge.
pub(in crate::cli::service_verify) async fn serving_standbys(
    directory: &ServiceDirectory,
    me: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, service) in &directory.services {
        let Some(endpoint) = service.standby.get(me) else {
            continue;
        };
        let (state, detail) = probe(&service.verification().kind, &endpoint.url).await;
        if state != OBSERVED {
            continue;
        }
        findings.push(Finding {
            service: name.clone(),
            host: me.to_string(),
            endpoint: endpoint.url.clone(),
            state: STANDBY_SERVING,
            detail: format!(
                "{detail}; {me} is a standby for {name}, which is active on {}, so this is a \
                 second copy serving beside it",
                service.active_host
            ),
            probed: true,
        });
    }
    findings
}

/// Replace the listed standby rows that a probe found serving, so a host
/// appears once per service: as the failure, not also as the quiet listing.
pub(in crate::cli::service_verify) fn merge_serving(listed: &mut Vec<Finding>, serving: Vec<Finding>) {
    listed.retain(|row| {
        !serving
            .iter()
            .any(|found| found.service == row.service && found.host == row.host)
    });
    listed.extend(serving);
}
