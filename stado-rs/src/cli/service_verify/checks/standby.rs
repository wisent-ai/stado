//! Addresses declared for a move that has not happened: listed everywhere,
//! and dialled only from the standby host itself, where an answer means a
//! second copy is serving.

use crate::deploy::service_serving::{PORT_SERVED_BY_OTHER, PORT_SERVED_BY_UNIT};
use crate::observations::{OBSERVED, STANDBY_SERVING, UNVERIFIED};
use crate::targets::{Registry, ServiceDirectory};

use crate::cli::service_verify::finding::STANDBY_DETAIL;
use crate::cli::service_verify::probe::probe;
use crate::cli::service_verify::verdicts::ownership::port_verdicts;
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
/// already lists the address. An answer alone proves only that something is
/// listening, so the port's owner is read the way `judge_ownership` reads it:
/// held by the unit the registry declares for the service on this host, it is
/// `standby_serving`, a second copy keeping its own state beside the active
/// host. Held by another job, or by an owner that cannot be established, it
/// stays an unprobed standby row whose detail says what answered, because an
/// unrelated listener is not a second copy of the service.
pub(in crate::cli::service_verify) async fn serving_standbys(
    registry: &Registry,
    directory: &ServiceDirectory,
    me: &str,
) -> Vec<Finding> {
    let runner = crate::deploy::production_runner();
    let mut findings = Vec::new();
    for (name, service) in &directory.services {
        let Some(endpoint) = service.standby.get(me) else {
            continue;
        };
        let (state, detail) = probe(&service.verification().kind, &endpoint.url).await;
        if state != OBSERVED {
            continue;
        }
        let port = url::Url::parse(&endpoint.url).ok().and_then(|url| url.port());
        let owner = match port {
            Some(port) => port_verdicts(registry, name, me, port, &runner).await,
            None => Err(format!("{} names no port to judge", endpoint.url)),
        };
        let (state, probed, detail) = match owner {
            Ok((unit, verdicts)) if verdicts.iter().any(|v| v.verdict == PORT_SERVED_BY_UNIT) => (
                STANDBY_SERVING,
                true,
                format!(
                    "{detail}; {me} is a standby for {name}, which is active on {}, and its own \
                     unit {unit} holds this port, so a second copy is serving beside it",
                    service.active_host
                ),
            ),
            Ok((unit, verdicts)) => {
                let holder = verdicts
                    .iter()
                    .find(|v| v.verdict == PORT_SERVED_BY_OTHER)
                    .map(|v| format!("held by {}", v.holder_cell()));
                (
                    UNVERIFIED,
                    false,
                    format!(
                        "{STANDBY_DETAIL}; something answered ({detail}) whose owner is not the \
                         standby unit {unit}: {}",
                        holder.unwrap_or_else(|| "owner could not be established".to_string())
                    ),
                )
            }
            Err(unjudged) => (
                UNVERIFIED,
                false,
                format!("{STANDBY_DETAIL}; something answered ({detail}), {unjudged}"),
            ),
        };
        findings.push(Finding {
            service: name.clone(),
            host: me.to_string(),
            endpoint: endpoint.url.clone(),
            state,
            detail,
            probed,
        });
    }
    findings
}

/// Replace the listed standby rows that a probe found serving, so a host
/// appears once per service: as the failure, not also as the quiet listing.
pub(in crate::cli::service_verify) fn merge_serving(
    listed: &mut Vec<Finding>,
    serving: Vec<Finding>,
) {
    listed.retain(|row| {
        !serving
            .iter()
            .any(|found| found.service == row.service && found.host == row.host)
    });
    listed.extend(serving);
}
