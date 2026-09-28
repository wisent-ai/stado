//! The service reconciler's second look at one standby before it stops it.
//!
//! The sweep that nominated the standby may be minutes old, so the look is
//! taken again under the unit's mutation lease, from the registry authority
//! and from the standby host itself. Three answers, never folded together:
//! still serving, positively settled (nothing to stop), or not judged (the
//! authority or the host could not be asked, so nothing may be concluded).

use crate::observations::{STANDBY_SERVING, UNVERIFIED};

use crate::cli::service_verify::checks::standby::serving_standbys;
use crate::cli::service_verify::finding::STANDBY_DETAIL;
use crate::cli::service_verify::probe::remote::remote_rows;
use crate::cli::service_verify::Finding;

/// What [`standby_still_serving`] found under the reconciler's lease.
pub(crate) enum StandbyRecheck {
    /// Still a standby, and its declared unit still holds the port.
    Serving,
    /// Positively nothing to stop: promoted, withdrawn, redeclared, or the
    /// standby host reports its standby address silent.
    Settled(String),
    /// The authority or the standby host could not be asked, or something
    /// answers whose owner was not established: nothing may be concluded.
    Unjudged(String),
}

/// Is `host` still a standby for `service`, with the declared `unit` serving
/// on the standby address?
///
/// The document is read from the authority, uncached, with no last-known-good
/// or bundled copy, because a copy of any age can name a host that has since
/// been promoted. This host is probed directly; another host through its own
/// `service verify --local`, the one vantage from which a standby's owner can
/// be judged. `Settled` needs positive evidence: the registry changed, or the
/// standby host answered and listed the address as silent.
pub(crate) async fn standby_still_serving(service: &str, host: &str, unit: &str) -> StandbyRecheck {
    let registry = match crate::targets::fetch_registry_authoritative().await {
        Ok(registry) => registry,
        Err(error) => {
            return StandbyRecheck::Unjudged(format!(
                "the registry authority did not answer: {error}"
            ))
        }
    };
    if registry.service_unit(service, host) != Some(unit) {
        return StandbyRecheck::Settled(format!(
            "the registry no longer declares {unit} for {service} on {host}"
        ));
    }
    let Some(directory) = registry.service_directory.as_ref() else {
        return StandbyRecheck::Settled("the registry declares no service directory".to_string());
    };
    let Some(endpoint) = directory
        .services
        .get(service)
        .filter(|declared| declared.active_host != host)
        .and_then(|declared| declared.standby.get(host))
        .map(|standby| standby.url.clone())
    else {
        return StandbyRecheck::Settled(format!("{host} is no longer a standby for {service}"));
    };
    let me = registry
        .lookup_self(&crate::providers::vast::system_hostname())
        .ok()
        .flatten()
        .map(|target| target.name.clone());
    let target = Standby {
        service,
        host,
        unit,
        endpoint: &endpoint,
    };
    if me.as_deref() == Some(host) {
        // Probed here: an address that did not answer yields no row at all.
        let rows = serving_standbys(&registry, directory, host).await;
        return judge(&rows, &target, true);
    }
    match remote_rows(host).await {
        Ok(rows) => judge(&rows, &target, false),
        Err(reason) => StandbyRecheck::Unjudged(format!(
            "{host} could not be asked about {service}: {reason}"
        )),
    }
}

/// The one standby declaration being re-checked, as the authority states it.
struct Standby<'a> {
    service: &'a str,
    host: &'a str,
    unit: &'a str,
    endpoint: &'a str,
}

/// Read the standby host's rows for exactly this declaration: the service,
/// the host and the standby address the authority names. A host that also
/// consumes the service reports that endpoint in its own rows, which say
/// nothing about the standby copy, so they are never read here. A local probe
/// files no row for a silent address; a remote host lists it as the plain
/// standby row.
fn judge(rows: &[Finding], standby: &Standby<'_>, local: bool) -> StandbyRecheck {
    let Standby {
        service,
        host,
        unit,
        endpoint,
    } = *standby;
    let matching: Vec<&Finding> = rows
        .iter()
        .filter(|row| row.service == service && row.host == host && row.endpoint == endpoint)
        .collect();
    if matching.iter().any(|row| row.state == STANDBY_SERVING) {
        return StandbyRecheck::Serving;
    }
    let Some(row) = matching.first() else {
        return if local {
            StandbyRecheck::Settled(format!("{host}'s standby address {endpoint} is silent"))
        } else {
            StandbyRecheck::Unjudged(format!(
                "{host}'s stado reported nothing for its {service} standby address {endpoint}"
            ))
        };
    };
    if row.state == UNVERIFIED && !row.probed && row.detail == STANDBY_DETAIL {
        return StandbyRecheck::Settled(format!(
            "{host}'s standby unit {unit} for {service} no longer holds {endpoint}"
        ));
    }
    StandbyRecheck::Unjudged(format!(
        "{host}'s standby address {endpoint} for {service}: {} ({})",
        row.state, row.detail
    ))
}
