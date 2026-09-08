//! `service endpoint-check` and `service serving`: the endpoint a unit is
//! declared to answer on, against the process that actually holds the port.
//!
//! Both commands resolve a name the same way — the label a host declares
//! first, then the service directory's own key for it — because they were
//! asked about the same service by the same operator on the same day and
//! refused it with two different sentences.

use super::*;

pub(crate) mod endpoint_check;
pub(crate) mod report;

pub(crate) struct ServingOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) ports: &'a [u16],
    pub(crate) as_json: bool,
}

/// The loopback port the service directory declares for this service on this
/// host, when it declares one.
///
/// This is the fleet's own statement of what the service answers on, which is
/// the only source that distinguishes a port a unit SERVES from one it merely
/// calls. `host_precheck_runner` reads the directory the same way.
///
/// `name` may be either the directory's own key for the service or the launchd
/// label the host declares for it, because this command accepts both and used
/// to resolve the endpoint for neither: `declared_matching` matches the host's
/// labels, this looked the same string up as a directory key, and no argument
/// satisfied both at once. Asked by service name it refused with "is not a
/// registry-managed service"; asked by label, with "the service directory
/// declares no endpoint" -- so the declared port of the service whose
/// declaration was wrong was the one port an operator had to supply by hand.
async fn directory_port(name: &str, host: &str) -> Option<u16> {
    let registry = host_channel::canonical_registry().await.ok()?;
    let key = if registry.service(name).is_some() {
        name
    } else {
        registry.service_named_by_unit(name, host)?
    };
    let endpoint = registry.service(key)?.address_for(host)?;
    url::Url::parse(&endpoint.url).ok()?.port()
}

/// The host's declaration for NAME, accepting the directory's own key for the
/// service as well as the launchd label the host declares.
///
/// [`declared_matching`] matches only the labels a host declares, while
/// `service verify`'s ownership judgement resolves the unit through
/// [`crate::targets::Registry::service_unit`] — which reads `managed_service`
/// and, when a placement profile owns the service instead, that profile's
/// `units` map. The two disagreed: on 2026-09-01 `service verify` judged
/// brama's port by label while `service serving brama` refused with "is not a
/// registry-managed service" on both hosts, because brama carries a
/// `placement_profile` and no `managed_service`. The command #248 points an
/// operator at could not answer for the one service the check was written
/// for. One resolution chain, both commands.
///
/// The label path is tried first so a host that declares a unit under a name
/// the directory also uses keeps resolving to its own declaration.
async fn declared_for_serving(name: &str, host: &str) -> Result<Vec<ManagedService>, CmdError> {
    let refusal = match declared_matching(name, Some(host)).await {
        Ok(found) => return Ok(found),
        Err(refusal) => refusal,
    };
    let Ok(registry) = host_channel::canonical_registry().await else {
        return Err(refusal);
    };
    let Some(unit) = registry.service_unit(name, host) else {
        return Err(refusal);
    };
    declared_matching(unit, Some(host)).await
}
