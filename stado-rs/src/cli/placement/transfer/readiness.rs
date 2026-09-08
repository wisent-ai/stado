//! What must hold before a move starts, and what routing must say once it
//! has: the loopback health probe, the routing units, and the preflight that
//! refuses a profile whose source or destination is not in the state the
//! transaction assumes.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::state::state_exists;
use super::units::{act_on_unit, probe_unit, UnitAction};
use super::{marker_line, run_host_script, MoveContext};
use crate::cli::placement::candidates::{
    ensure_profile_lifecycle_mutable, managed_unit, profile_host, target, unit,
};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

pub(super) async fn health_probe(
    target: &ComputeTarget,
    url: &str,
    attempts: usize,
    runner: &Runner,
) -> Result<(), CmdError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::click(format!("invalid placement probe URL: {error}")))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if parsed.scheme() != "http" || !loopback || parsed.port().is_none() {
        return Err(CmdError::click(format!(
            "placement probe must use loopback HTTP with an explicit port: {url}"
        )));
    }
    let url_payload = STANDARD.encode(url.as_bytes());
    let attempts = attempts.max(1);
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
url=$(printf '%s' '{url_payload}' | /usr/bin/base64 "$decode")
attempt=0
while [ "$attempt" -lt '{attempts}' ]; do
  attempt=$((attempt + 1))
  status=$(/usr/bin/curl --silent --show-error --max-time 10 --output /dev/null --write-out '%{{http_code}}' "$url" 2>/dev/null || true)
  case "$status" in 2??) printf 'STADO_PLACEMENT_HEALTH\tok\t%s\n' "$status"; exit 0 ;; esac
  [ "$attempt" -ge '{attempts}' ] || /bin/sleep 1
done
printf 'health probe failed: %s returned HTTP %s\n' "$url" "$status" >&2
exit 69
"#
    );
    let output = run_host_script(target, &script, runner, "health probe").await?;
    if marker_line(&output, "STADO_PLACEMENT_HEALTH\tok\t").is_none() {
        return Err(CmdError::click(format!(
            "{}: health probe returned no marker for {url}",
            target.name
        )));
    }
    Ok(())
}

pub(super) async fn apply_routes(
    context: &MoveContext,
    destination: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    for route in &context.profile.routing {
        let route_target = target(&context.registry, &route.host)?;
        let action = if route.active_when_destination == destination {
            UnitAction::Start
        } else {
            UnitAction::Retire
        };
        act_on_unit(route_target, &route.unit, action, runner).await?;
    }
    Ok(())
}

pub(super) async fn preflight(context: &MoveContext, runner: &Runner) -> Result<(), CmdError> {
    ensure_profile_lifecycle_mutable(&context.profile)?;
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        let source_unit = managed_unit(source_spec)?;
        let source_status = probe_unit(&context.source, source_spec, runner).await?;
        if !source_status.present || !source_status.loaded {
            return Err(CmdError::click(format!(
                "{}: source unit {} must be installed and running before migration",
                context.source.name, source_unit.unit
            )));
        }
        let destination_spec = unit(destination_profile, logical)?;
        let destination_unit = managed_unit(destination_spec)?;
        let destination_status = probe_unit(&context.destination, destination_spec, runner).await?;
        if !destination_status.present {
            return Err(CmdError::click(format!(
                "{}: destination unit file is missing: {}",
                context.destination.name, destination_unit.path
            )));
        }
        if destination_status.loaded {
            return Err(CmdError::click(format!(
                "{}: destination unit {} is already running; refusing two active copies",
                context.destination.name, destination_unit.unit
            )));
        }
    }
    if !context.profile.allow_unhealthy_source {
        for probe in &source_profile.probes {
            health_probe(&context.source, &probe.url, 1, runner).await?;
        }
    }
    for state in &context.profile.state {
        let exists = state_exists(&context.source, state, runner).await?;
        if state.required && !exists {
            return Err(CmdError::click(format!(
                "{}: required state file is missing: $HOME/{}",
                context.source.name, state.path
            )));
        }
    }
    for route in &context.profile.routing {
        let route_target = target(&context.registry, &route.host)?;
        let route_unit = managed_unit(&route.unit)?;
        let status = probe_unit(route_target, &route.unit, runner).await?;
        if !status.present {
            return Err(CmdError::click(format!(
                "{}: routing unit file is missing: {}",
                route.host, route_unit.path
            )));
        }
    }
    Ok(())
}
