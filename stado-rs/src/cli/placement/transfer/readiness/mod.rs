//! What must hold before a move starts, and what routing must say once it
//! has: the loopback health probe (in `probe`), the routing units, and the
//! preflight that refuses a profile whose source or destination is not in
//! the state the transaction assumes.

mod probe;

pub(super) use probe::await_healthy;
use probe::health_probe;

use super::cutover::promote_standby;
use super::state::{ensure_room, root_payload, state_exists, state_location};
use super::units::{act_on_unit, probe_unit, UnitAction};
use super::MoveContext;
use crate::cli::placement::candidates::{
    ensure_profile_lifecycle_mutable, managed_unit, profile_host, target, unit,
};
use crate::cli::CmdError;
use crate::deploy::Runner;

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

/// Stop every destination unit of the profile that runs: the preflight let
/// such a copy through only when the destination holds none of the profile's
/// required state. Stopped while the source still serves, so the state the
/// move writes next is what the started unit opens. The units it stopped.
pub(super) async fn stop_empty_copies(
    context: &MoveContext,
    runner: &Runner,
) -> Result<Vec<String>, CmdError> {
    let destination = profile_host(&context.profile, &context.destination.name)?;
    let mut stopped = Vec::new();
    for logical in &context.profile.stop_order {
        let spec = unit(destination, logical)?;
        if probe_unit(&context.destination, spec, runner).await?.loaded {
            act_on_unit(&context.destination, spec, UnitAction::Stop, runner).await?;
            stopped.push(logical.clone());
        }
    }
    Ok(stopped)
}

/// Every store the catalog declares for a product of the profile
/// (`service.state`) is one of the profile's states. A profile declared
/// before the catalog named the store, or written by hand without it, moved
/// the unit and left what the service keeps on the fenced source, where the
/// moved service could not find it.
fn ensure_declared_stores(
    profile: &crate::placement::PlacementProfile,
    source: &crate::placement::PlacementHost,
) -> Result<(), CmdError> {
    use crate::cli::placement::profile::stores::{listed, missing_stores};
    let missing = missing_stores(profile, source)?;
    if missing.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "placement profile {} does not carry {}, which the catalog declares its products \
         keep; a move would leave them behind on the source: add them with stado placement \
         profile sync {}",
        profile.name,
        listed(&missing),
        profile.name
    )))
}

/// Destination units already running are a second active copy, refused,
/// unless the destination holds none of the profile's required state: a
/// copy started with nothing (a standby someone loaded, a unit a host
/// install started) has accepted nothing that a move would overwrite, so the
/// move stops it before it writes the state. A profile that declares no
/// required state cannot tell an empty copy, and is refused as before.
async fn ensure_empty_copies(
    context: &MoveContext,
    running: &[String],
    runner: &Runner,
) -> Result<(), CmdError> {
    if running.is_empty() {
        return Ok(());
    }
    let units = running.join(", ");
    let required: Vec<_> = context
        .profile
        .state
        .iter()
        .filter(|state| state.required)
        .collect();
    if required.is_empty() {
        return Err(CmdError::refused(format!(
            "{}: destination unit(s) {units} already run, and the profile declares no required \
             state to tell an empty copy by; refusing two active copies",
            context.destination.name
        )));
    }
    let held = required
        .iter()
        .map(|state| state_exists(&context.destination, state, runner));
    for (state, exists) in required.iter().zip(futures::future::join_all(held).await) {
        if exists? {
            return Err(CmdError::refused(format!(
                "{}: destination unit(s) {units} already run and hold {}; refusing two active \
                 copies",
                context.destination.name,
                state_location(state)
            )));
        }
    }
    println!(
        "  {}: unit(s) {units} run with none of the profile's required state; the move stops \
         them before it writes the state",
        context.destination.name
    );
    Ok(())
}

/// Every check a move makes before it fences the source, each a read: the
/// units on both hosts, the source's health, its states, the destination's
/// room and the routing units. `placement move --check` runs it alone.
pub(in crate::cli::placement) async fn preflight(
    context: &MoveContext,
    runner: &Runner,
) -> Result<(), CmdError> {
    ensure_profile_lifecycle_mutable(&context.profile)?;
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;
    ensure_declared_stores(&context.profile, source_profile)?;
    // The cutover gives the destination its endpoint from the directory; a
    // destination it cannot name is refused here, before the source is fenced,
    // rather than at the commit after every unit already moved.
    promote_standby(
        &mut context.claimed_document.clone(),
        &context.profile.name,
        &context.destination.name,
    )?;

    // Whether anything of this profile is actually running on the source.
    // A profile whose units are all stopped is not a reason to refuse the
    // move; it is the state a host in trouble leaves behind.
    let mut source_running = false;
    let mut running_copies: Vec<String> = Vec::new();
    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        let source_unit = managed_unit(source_spec)?;
        let destination_spec = unit(destination_profile, logical)?;
        let destination_unit = managed_unit(destination_spec)?;
        // The two hosts are asked at once: each probe is a channel to its own
        // host, and a slow source no longer delays the destination's answer.
        // The source's failure is still the one reported first.
        let (source_status, destination_status) = futures::join!(
            probe_unit(&context.source, source_spec, runner),
            probe_unit(&context.destination, destination_spec, runner)
        );
        let source_status = source_status?;
        let destination_status = destination_status?;
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
        if !destination_status.present {
            return Err(CmdError::click(format!(
                "{}: destination unit file is missing: {}",
                context.destination.name, destination_unit.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
        if destination_status.loaded {
            running_copies.push(destination_unit.unit.clone());
        }
    }
    // A health probe against a source with nothing running answers nothing
    // about the move: it fails because the service is down, which is the
    // condition being relieved. It is skipped for exactly that case and kept
    // for every other, so a live source still has to be healthy — or declare
    // `allow_unhealthy_source` — before it is fenced.
    // The probes, the state files and the routing units below are each their
    // own question to a host, so each group is asked at once; the first
    // refusal in the profile's order is the one reported.
    if !context.profile.allow_unhealthy_source && source_running {
        let probes = source_profile
            .probes
            .iter()
            .map(|probe| health_probe(&context.source, &probe.url, runner));
        for answer in futures::future::join_all(probes).await {
            answer?;
        }
    }
    // A work-rooted state needs a declared work root on both sides; the
    // destination is asked here, before the source is fenced.
    for state in &context.profile.state {
        root_payload(&context.destination, state)?;
    }
    let present = context
        .profile
        .state
        .iter()
        .map(|state| state_exists(&context.source, state, runner));
    let present = futures::future::join_all(present).await;
    for (state, exists) in context.profile.state.iter().zip(present) {
        // A read that failed is the refusal for every state, required or not.
        let exists = exists?;
        if state.required && !exists {
            return Err(CmdError::click(format!(
                "{}: required state file is missing: {}",
                context.source.name,
                state_location(state)
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
    }
    ensure_empty_copies(context, &running_copies, runner).await?;
    // The destination must hold what the move writes before the source is
    // fenced; a full volume refused the move only after its first write.
    ensure_room(context, runner).await?;
    let mut routes = Vec::with_capacity(context.profile.routing.len());
    for route in &context.profile.routing {
        let route_target = target(&context.registry, &route.host)?;
        routes.push((route, route_target, managed_unit(&route.unit)?));
    }
    let statuses = routes
        .iter()
        .map(|(route, route_target, _)| probe_unit(route_target, &route.unit, runner));
    let statuses = futures::future::join_all(statuses).await;
    for ((route, _, route_unit), status) in routes.iter().zip(statuses) {
        if !status?.present {
            return Err(CmdError::click(format!(
                "{}: routing unit file is missing: {}",
                route.host, route_unit.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
    }
    Ok(())
}
