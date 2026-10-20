//! What must hold before a move starts, and what routing must say once it
//! has: the loopback health probe, the routing units, and the preflight that
//! refuses a profile whose source or destination is not in the state the
//! transaction assumes.
//!
//! A probe ends on facts, never on a count or a clock: a 2xx answer is
//! healthy; a destination unit that no longer runs is not, named with what
//! its URL last answered; anything else, while the unit runs, is read again.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::state::{root_payload, state_exists, state_location};
use super::units::{act_on_unit, probe_unit, UnitAction};
use super::{marker_line, run_host_script, MoveContext};
use crate::cli::placement::candidates::{
    ensure_profile_lifecycle_mutable, managed_unit, profile_host, target, unit,
};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementUnit;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// What the service at a probe URL answered once: its HTTP status as curl
/// writes it (`000` when nothing answered), and curl's exit status and error
/// when the request did not complete.
struct Answer {
    status: String,
    failure: Option<String>,
}

impl Answer {
    fn healthy(&self) -> bool {
        self.failure.is_none() && self.status.starts_with('2')
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            Some(failure) => write!(f, "no HTTP answer ({failure})"),
            None => write!(f, "HTTP {}", self.status),
        }
    }
}

/// A probe URL is loopback HTTP with an explicit port, as the profile
/// contract declares; its bytes travel to the host as base64.
fn probe_payload(url: &str) -> Result<String, CmdError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::declaration(format!("invalid placement probe URL: {error}")))?;
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    };
    if parsed.scheme() != "http" || !loopback || parsed.port().is_none() {
        return Err(CmdError::click(format!(
            "placement probe must use loopback HTTP with an explicit port: {url}"
        ))
        .stating(FailureCode::Config));
    }
    Ok(STANDARD.encode(url.as_bytes()))
}

/// One request to the probe URL on `target`, answered whatever it returned.
async fn answer(
    target: &ComputeTarget,
    url: &str,
    payload: &str,
    runner: &Runner,
) -> Result<Answer, CmdError> {
    // curl reports a request that did not complete through --write-out
    // (exitcode, errormsg), so its own exit status is read there.
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
url=$(printf '%s' '{payload}' | /usr/bin/base64 "$decode")
/usr/bin/curl --silent --output /dev/null --write-out 'STADO_PLACEMENT_HEALTH\t%{{http_code}}\t%{{exitcode}}\t%{{errormsg}}\n' "$url" || :
"#
    );
    let output = run_host_script(target, &script, runner, "health probe").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_HEALTH\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: health probe returned no marker for {url}",
            target.name
        ))
        .stating(FailureCode::InfraDown)
    })?;
    let fields: Vec<&str> = line.split('\t').collect();
    let [_, status, exit, message] = fields.as_slice() else {
        return Err(CmdError::click(format!(
            "{}: malformed health probe marker for {url}: {line}",
            target.name
        ))
        .stating(FailureCode::InfraDown));
    };
    Ok(Answer {
        status: status.to_string(),
        failure: (!message.is_empty()).then(|| format!("curl exit {exit}: {message}")),
    })
}

/// The service at `url` on a running source answers 2xx now; any other
/// answer refuses the move before the source is fenced, naming it.
pub(super) async fn health_probe(
    target: &ComputeTarget,
    url: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let payload = probe_payload(url)?;
    let answer = answer(target, url, &payload, runner).await?;
    if answer.healthy() {
        return Ok(());
    }
    Err(
        CmdError::click(format!("{}: {url} answered {answer}", target.name))
            .stating(FailureCode::InfraDown),
    )
}

/// The started unit `spec` on `target` answers 2xx at `url`. Until it does,
/// the URL is read again while the unit's process runs; once it no longer
/// runs, the move is refused naming the unit and the URL's last answer.
pub(super) async fn await_healthy(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    url: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let payload = probe_payload(url)?;
    let managed = managed_unit(spec)?;
    loop {
        let answer = answer(target, url, &payload, runner).await?;
        if answer.healthy() {
            return Ok(());
        }
        if !probe_unit(target, spec, runner).await?.running {
            return Err(CmdError::click(format!(
                "{}: unit {} is no longer running; {url} last answered {answer}",
                target.name, managed.unit
            ))
            .stating(FailureCode::InfraDown));
        }
    }
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

    // Whether anything of this profile is actually running on the source.
    // A profile whose units are all stopped is not a reason to refuse the
    // move; it is the state a host in trouble leaves behind.
    let mut source_running = false;
    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        let source_unit = managed_unit(source_spec)?;
        let source_status = probe_unit(&context.source, source_spec, runner).await?;
        // The unit FILE is the declaration this transaction moves, so its
        // absence is still a refusal: there is nothing here to relocate.
        if !source_status.present {
            return Err(CmdError::click(format!(
                "{}: source unit file is missing: {}",
                context.source.name, source_unit.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
        if source_status.loaded {
            source_running = true;
        } else {
            // Said, not hidden: the operator reading the move receipt has to
            // know the service was already down when it was relocated, and
            // the fencing step below is a no-op for it.
            println!(
                "  {}: source unit {} is installed and not running; the move carries its \
                 declaration and its state",
                context.source.name, source_unit.unit
            );
        }
        let destination_spec = unit(destination_profile, logical)?;
        let destination_unit = managed_unit(destination_spec)?;
        let destination_status = probe_unit(&context.destination, destination_spec, runner).await?;
        if !destination_status.present {
            return Err(CmdError::click(format!(
                "{}: destination unit file is missing: {}",
                context.destination.name, destination_unit.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
        if destination_status.loaded {
            return Err(CmdError::refused(format!(
                "{}: destination unit {} is already running; refusing two active copies",
                context.destination.name, destination_unit.unit
            )));
        }
    }
    // A health probe against a source with nothing running answers nothing
    // about the move: it fails because the service is down, which is the
    // condition being relieved. It is skipped for exactly that case and kept
    // for every other, so a live source still has to be healthy — or declare
    // `allow_unhealthy_source` — before it is fenced.
    if !context.profile.allow_unhealthy_source && source_running {
        for probe in &source_profile.probes {
            health_probe(&context.source, &probe.url, runner).await?;
        }
    }
    for state in &context.profile.state {
        // A work-rooted state needs a declared work root on both sides; the
        // destination is asked here, before the source is fenced.
        root_payload(&context.destination, state)?;
        let exists = state_exists(&context.source, state, runner).await?;
        if state.required && !exists {
            return Err(CmdError::click(format!(
                "{}: required state file is missing: {}",
                context.source.name,
                state_location(state)
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
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
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
    }
    Ok(())
}
