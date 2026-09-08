//! Which origin a configured value names, and the canonical one release
//! reads go to.

use crate::cli::storage::*;

fn validated_object_base_url(variable: &str, value: &str) -> Result<Option<url::Url>, CmdError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let url = url::Url::parse(value)
        .map_err(|error| CmdError::click(format!("invalid {variable}: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(CmdError::click(format!(
            "{variable} must be an absolute HTTP or HTTPS URL"
        )));
    }
    let host = url.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !loopback {
        return Err(CmdError::click(format!(
            "{variable} must use HTTPS unless its host is loopback"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CmdError::click(format!(
            "{variable} must not contain embedded credentials"
        )));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CmdError::click(format!(
            "{variable} must not contain a query string or fragment"
        )));
    }
    Ok(Some(url))
}

pub(in crate::cli::storage) fn configured_object_base_url(
    variable: &str,
) -> Result<Option<url::Url>, CmdError> {
    let value = match std::env::var(variable) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(CmdError::click(format!("{variable} must be valid Unicode")));
        }
    };
    validated_object_base_url(variable, &value)
}

/// The canonical origin from the environment, and then from `api.url`.
///
/// `STADO_API_URL` is a configuration field, not an environment-only switch:
/// `config::stado_api_url` resolves both, and that is how the scheduler, the
/// doctor and every enrolment path read it. Reading the environment alone made
/// `stado host release` refuse a fleet delivery with "STADO_API_URL is
/// required for canonical release reads" on a host whose own configuration
/// declared the canonical origin — printed back by `host config-show` while
/// being refused.
///
/// Only the release channel resolves it this way. The private object plane
/// keeps its own endpoint: widening the shared reader instead sent every
/// object write to the public origin, and a source archive PUT there answered
/// `504 FUNCTION_INVOCATION_TIMEOUT` twice before the cause was the diff.
pub(in crate::cli::storage) fn configured_api_origin() -> Result<Option<url::Url>, CmdError> {
    if let Some(url) = configured_object_base_url("STADO_API_URL")? {
        return Ok(Some(url));
    }
    validated_object_base_url("api.url", &crate::config::stado_api_url())
}

/// Canonical public origin for immutable release reads. Release consumers use
/// the same `STADO_API_URL` contract as `storage get|stat|url`; there is no
/// release-specific origin that can drift from it.
///
/// HTTPS is the rule because the origin leaves the machine asking. The one
/// exception is loopback HTTP: the delivery fetch runs on the target itself,
/// so a loopback origin can only name that host's own store — self-delivery,
/// with no network path to tamper with. [`plan`](crate::deploy::host_release::plan)
/// keeps the per-target gate: it accepts this shape only for the host the
/// service directory says serves the object API.
pub(crate) fn release_api_origin() -> Result<String, CmdError> {
    let url = configured_api_origin()?
        .ok_or_else(|| CmdError::click("STADO_API_URL is required for canonical release reads"))?;
    if url.scheme() != "https" && !crate::deploy::host_release::loopback_http_origin(url.as_str()) {
        return Err(CmdError::click(
            "STADO_API_URL must use HTTPS for delivery to fleet hosts; loopback HTTP is allowed \
             only when the target is its own release store",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}
