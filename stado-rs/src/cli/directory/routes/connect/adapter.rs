//! The route a machine's own resolver adapter gives one caller of a service,
//! and the details a refused route carries.

use serde_json::{json, Value};

use crate::cli::CmdError;

pub(super) fn route_failure(
    error: CmdError,
    service: &str,
    consumer: Option<&str>,
    state: &str,
) -> CmdError {
    error.detailing(json!({
        "service": service,
        "consumer": consumer,
        "machine": crate::providers::vast::system_hostname(),
        "route_state": state,
    }))
}

/// The loopback address `asking` declares for reaching `service`, or `None` when
/// that machine declares no resolver or no adapter for it. A resolver
/// declaration that does not parse, or an asking machine the registry does
/// not hold, is the error: read as no adapter, the caller was sent to the
/// service's host directly past a resolver that was declared all along.
///
/// Read from `registry.targets[<asking>].service_resolver.adapters`, the same
/// declaration the resolver itself binds. A machine that consumes a service
/// through the resolver has exactly one bind per consumer, so naming the
/// consumer is what disambiguates two programs sharing a service: `brama` on an
/// operator laptop is bound once for `operator` and once for `brama-desktop`,
/// and handing the wrong one to a caller puts it on a channel whose idle and
/// connect budgets belong to somebody else.
pub(super) fn adapter_route(
    document: &Value,
    asking: &str,
    service: &str,
    consumer: Option<&str>,
    scheme: &str,
) -> Result<Option<String>, CmdError> {
    let invalid = |error| route_failure(error, service, consumer, "invalid_declaration");
    let Some(config) = crate::service_resolution::declared_resolver_config(document, asking)
        .map_err(|error| invalid(CmdError::declaration(error)))?
    else {
        return Ok(None);
    };
    let mut declared: Vec<&crate::service_resolution::ResolverAdapter> = config
        .adapters
        .iter()
        .filter(|adapter| adapter.service == service)
        .collect();
    if let Some(consumer) = consumer {
        declared.retain(|adapter| adapter.consumer == consumer);
        if declared.is_empty() {
            return Err(invalid(CmdError::missing(format!(
                "{asking} declares no resolver adapter for {service} as consumer {consumer}; \
                 declare one in registry.targets[{asking}].service_resolver.adapters"
            ))));
        }
    }
    match declared.as_slice() {
        [] => Ok(None),
        [adapter] => Ok(Some(format!("{scheme}://{}", adapter.bind))),
        several => Err(CmdError::usage(format!(
            "{asking} declares {} resolver adapters for {service}, one per consumer ({}); \
             name the caller with --consumer",
            several.len(),
            several
                .iter()
                .map(|adapter| adapter.consumer.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}
