//! `stado placement take SERVICE... --reason WHY`, run on the host that
//! takes the profile: a placement profile taken from an active host that
//! does not answer.
//!
//! `placement move` stops the source's units, copies the profile's states
//! and starts them on the destination, so every step needs the source to
//! answer. When the source is gone (switched off, asleep, off the network)
//! each of those steps stands until its caller gives up, the profile stays
//! placed on a host nobody reaches, and every consumer of its services waits
//! on it. `placement role take` is the answer for a role of the host Stado
//! process; this is the same answer for a profile.
//!
//! Every SSH path of the source is probed once, each under the channel's
//! own connect bound; a path that answers refuses the take (the profile
//! moves with its states, `placement move`). SSH closed is not the units
//! stopped, so each probe URL the profile declares for the source is then
//! asked at every network name the source is known by ([`still_serving`]),
//! and an answer refuses the take: a copy started here would serve beside
//! it. Then the destination is asked what it holds of each state: a
//! required state it lacks refuses the take, naming it, since the unit
//! would start with nothing; an optional one it lacks is named as not
//! recovered, and the age of every copy it holds is named, since every
//! write the source took after it is lost. Each unit starts on the
//! destination in the profile's start order, each probe must answer 2xx,
//! and one registry commit places the profile on the destination, the same
//! change a move commits ([`placed_on`]). The source's units are not
//! stopped, since nothing reaches them: the receipt names them, and whoever
//! brings the source back retires them before it serves again.

use std::time::Duration;

use serde_json::{json, Value};

use super::execution::placed_on;
use super::readiness::await_healthy;
use super::serves::still_serving;
use super::state::{state_exists, state_location, state_newest};
use super::units::{act_on_unit, UnitAction};
use crate::cli::placement::candidates::{parse_registry, profile_host, target, unit};
use crate::cli::{registry, CmdError};
use crate::deploy::{host_channel, production_runner, Runner};
use crate::placement::PlacementProfile;
use crate::targets::ComputeTarget;

/// How long ago `seconds` (Unix time) was, as `{modified, seconds_ago}`: the
/// writes the source took after it are not in this copy.
fn age_of(seconds: i64) -> Value {
    let ago = chrono::Utc::now().timestamp() - seconds;
    let modified = chrono::DateTime::from_timestamp(seconds, 0)
        .map(|moment| moment.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    json!({ "modified": modified, "seconds_ago": ago })
}

/// The path `source` answers on, when any declared path answers; each is
/// asked once, under the channel's own bound.
async fn answering_path(
    source: &ComputeTarget,
    runner: &Runner,
) -> Result<Option<String>, CmdError> {
    let probes = host_channel::probe_ssh_connections(source, runner)
        .await
        .map_err(CmdError::from)?;
    Ok(probes
        .into_iter()
        .find(|probe| probe.reachable)
        .map(|probe| format!("{} ({})", probe.name, probe.destination)))
}

/// The states of `profile` that `destination` lacks: the required ones,
/// then the optional ones, each by its location.
async fn lacking_states(
    profile: &PlacementProfile,
    destination: &ComputeTarget,
    runner: &Runner,
) -> Result<(Vec<String>, Vec<String>), CmdError> {
    let (mut required, mut optional) = (Vec::new(), Vec::new());
    for state in &profile.state {
        if state_exists(destination, state, runner).await? {
            continue;
        }
        match state.required {
            true => required.push(state_location(state)),
            false => optional.push(state_location(state)),
        }
    }
    Ok((required, optional))
}

/// Take `profile`, read from `document` at `generation`, on `here` from the
/// host it is placed on, which must answer on no path. With `check`,
/// everything is read and nothing started or written. The receipt.
pub(in crate::cli::placement) async fn take_profile(
    document: Value,
    generation: String,
    profile: PlacementProfile,
    here: &str,
    reason: &str,
    check: bool,
) -> Result<Value, CmdError> {
    let parsed = parse_registry(&document)?;
    let from =
        crate::cli::placement::placed_host(&parsed, &profile).map_err(CmdError::declaration)?;
    if from == here {
        return Err(CmdError::refused(format!(
            "{} is already placed on {here}",
            profile.name
        )));
    }
    let destination_units = profile_host(&profile, here)?.clone();
    let source = target(&parsed, &from)?.clone();
    let destination = target(&parsed, here)?.clone();
    let runner = production_runner();
    if let Some(path) = answering_path(&source, &runner).await? {
        return Err(CmdError::refused(format!(
            "{from}, where {} is placed, answers over {path}; move it with its states instead: \
             stado placement move {} --to-host {here}",
            profile.name,
            profile.services.join(" ")
        )));
    }
    // SSH closed says nothing about the units: a source still answering its
    // clients would serve beside the copy started here, each writing its own
    // state.
    let bound = crate::cli::resolver::declared_bound_seconds()
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::refused(
                "this host's resolver declares no refresh interval, so how long the source's \
                 services are given to answer cannot be said; declare it in this host's \
                 registry record, then take again"
                    .to_string(),
            )
        })?;
    if let Some(serving) = still_serving(&profile, &source, Duration::from_secs(bound)).await? {
        return Err(CmdError::refused(format!(
            "{from} still serves {serving}, although no SSH path of it answers; a copy started \
             on {here} would serve beside it and the two would write diverging state; stop it \
             on {from}, or move the profile once {from} answers over SSH"
        )));
    }
    // A routing unit on the silent source would have to retire there, which
    // nothing can do; one elsewhere is applied as a move applies it.
    if let Some(route) = profile.routing.iter().find(|route| route.host == from) {
        return Err(CmdError::refused(format!(
            "{} routes through {} on {from}, which does not answer, so the route cannot be \
             retired there; move the profile once {from} answers",
            profile.name, route.unit.name
        )));
    }
    let (missing, not_recovered) = lacking_states(&profile, &destination, &runner).await?;
    if !missing.is_empty() {
        return Err(CmdError::refused(format!(
            "{here} holds none of {}, which {} requires, and {from} does not answer to give it; \
             put it on {here} first, or move the profile once {from} answers",
            missing.join(", "),
            profile.name
        )));
    }
    let mut ages = serde_json::Map::new();
    let held = profile
        .state
        .iter()
        .filter(|state| !not_recovered.contains(&state_location(state)));
    for state in held {
        let newest = state_newest(&destination, state, &runner).await?;
        ages.insert(state_location(state), json!(newest.map(age_of)));
    }
    let unretired: Vec<String> = profile_host(&profile, &from)?
        .units
        .values()
        .map(|spec| spec.name.clone())
        .collect();
    let mut receipt = json!({
        "state": if check { "would_take" } else { "taken" },
        "profile": profile.name,
        "from_host": from,
        "to_host": here,
        "reason": reason,
        "state_age": ages,
        "not_recovered": not_recovered,
        "unretired_source": unretired,
    });
    if check {
        return Ok(receipt);
    }
    for route in &profile.routing {
        let action = match route.active_when_destination == here {
            true => UnitAction::Start,
            false => UnitAction::Retire,
        };
        act_on_unit(target(&parsed, &route.host)?, &route.unit, action, &runner).await?;
    }
    for logical in &profile.start_order {
        let spec = unit(&destination_units, logical)?;
        act_on_unit(&destination, spec, UnitAction::Start, &runner).await?;
    }
    for probe in &destination_units.probes {
        let spec = unit(&destination_units, &probe.service)?;
        await_healthy(&destination, spec, &probe.url, &runner).await?;
    }
    let placed = placed_on(document, &profile, &source, &destination)?;
    let committed = registry::push_document_if(&placed, &generation).await?;
    receipt["registry_generation"] = json!(committed);
    Ok(receipt)
}
