//! The ordered steps of one transaction: fence, transfer, route, start,
//! probe, retire, commit. The reverse of every step that already happened,
//! run on any failure before the commit, is [`rollback`].

use std::fmt::Display;
use std::future::Future;
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use super::cutover::promote_standby;
use super::readiness::{apply_routes, await_healthy, preflight, stop_empty_copies};
use super::state::{read_state, state_location, write_state};
use super::units::{act_on_unit, UnitAction};
use super::{MoveContext, Progress, RegistryCommitter, StepTiming};
use crate::cli::placement::candidates::{deploy_error, managed_unit, profile_host, unit};
use crate::cli::CmdError;
use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::Runner;
use crate::placement::{self, PlacementUnit};
use crate::targets::ComputeTarget;

mod rollback;

pub(in crate::cli::placement) use rollback::{release_claim, rollback};

fn destination_record(
    destination: &ComputeTarget,
    spec: &PlacementUnit,
    managed_since: &str,
) -> Result<ManagedService, CmdError> {
    let unit = managed_unit(spec)?;
    let mut managed = match unit.kind {
        crate::deploy::service::UnitKind::Launchd => service::launchd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        ),
        crate::deploy::service::UnitKind::Systemd => service::systemd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        ),
    };
    managed.name = spec.name.clone();
    managed.host_heuristic = destination.host_heuristic.clone();
    Ok(managed)
}

fn prepare_committed_document(context: &MoveContext) -> Result<Value, CmdError> {
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;
    let mut document = context.claimed_document.clone();
    let managed_since = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        let source_unit = managed_unit(source_spec)?;
        service::remove_service(&mut document, &context.source.name, &source_unit.unit)
            .map_err(deploy_error)?;
        let destination_spec = unit(destination_profile, logical)?;
        let managed = destination_record(&context.destination, destination_spec, &managed_since)?;
        service::add_service(&mut document, &managed).map_err(deploy_error)?;
    }
    promote_standby(
        &mut document,
        &context.profile.name,
        &context.destination.name,
    )?;
    crate::service_resolution::retarget_profile(
        &mut document,
        &context.profile.name,
        &context.destination.name,
    )
    .map_err(CmdError::declaration)?;
    if !placement::release_transaction(&mut document, &context.transaction.id)
        .map_err(CmdError::declaration)?
    {
        return Err(CmdError::refused(
            "placement transaction disappeared before commit",
        ));
    }
    Ok(document)
}

/// One step of the transaction on `host`, said as a wait before it starts
/// and measured when it ends ([`crate::wait::until`]). A move whose process
/// printed nothing for minutes could not tell the operator which step held
/// it, nor how long that step took the last time; each step is an open wait
/// naming the transaction, the profile and the host, `stado waits` lists the
/// one standing, and a later move's wait on the same step opens saying when
/// it is due. Its time is also added to `steps`, which the receipt lists, so a
/// move that ran long says where the time went.
async fn step<T>(
    context: &MoveContext,
    steps: &mut Vec<StepTiming>,
    host: &str,
    what: impl Display,
    work: impl Future<Output = Result<T, CmdError>>,
) -> Result<T, CmdError> {
    let what = what.to_string();
    let started = Instant::now();
    let result = crate::wait::until(
        crate::wait::Kind::Host,
        format!(
            "placement move {} of {}: {what}",
            context.transaction.id, context.profile.name
        ),
        host,
        work,
    )
    .await;
    steps.push(StepTiming {
        step: what,
        host: host.to_string(),
        seconds: started.elapsed().as_secs_f64(),
        succeeded: result.is_ok(),
    });
    result
}

/// Several independent steps on `host` at once, each its own wait and timed
/// like [`step`]; their times join `steps` in the order the steps were given,
/// and the first failure among them is the answer once every one has ended,
/// so no step is left running behind a failure the rollback acts on.
async fn steps_together<T, F>(
    context: &MoveContext,
    steps: &mut Vec<StepTiming>,
    host: &str,
    work: Vec<(String, F)>,
) -> Result<Vec<T>, CmdError>
where
    F: Future<Output = Result<T, CmdError>>,
{
    let timed = work.into_iter().map(|(what, future)| async move {
        let started = Instant::now();
        let result = crate::wait::until(
            crate::wait::Kind::Host,
            format!(
                "placement move {} of {}: {what}",
                context.transaction.id, context.profile.name
            ),
            host,
            future,
        )
        .await;
        (what, started.elapsed().as_secs_f64(), result)
    });
    let mut answers = Vec::new();
    let mut failure = None;
    for (what, seconds, result) in futures::future::join_all(timed).await {
        steps.push(StepTiming {
            step: what,
            host: host.to_string(),
            seconds,
            succeeded: result.is_ok(),
        });
        match result {
            Ok(answer) => answers.push(answer),
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(answers),
    }
}

pub(in crate::cli::placement) async fn execute_move(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
    committer: &RegistryCommitter,
) -> Result<String, CmdError> {
    let both = format!("{} and {}", context.source.name, context.destination.name);
    let checked = preflight(context, runner);
    step(context, &mut progress.steps, &both, "preflight", checked).await?;
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    println!(
        "moving {}: {} -> {}",
        context.profile.name, context.source.name, context.destination.name
    );
    let emptied = stop_empty_copies(context, runner);
    for logical in step(context, &mut progress.steps, &context.destination.name, "stop empty copies", emptied).await? {
        println!("  stopped the empty copy {}:{}", context.destination.name, logical);
    }
    // Every step counts as taken, in memory and in the journal on this host,
    // before it starts: a rollback or a later release then undoes a step that
    // was cut short, and never one that did not begin.
    progress.source_stopped = true;
    super::journal::keep(&context.transaction.id, progress);
    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        let fence = act_on_unit(&context.source, spec, UnitAction::Stop, runner);
        let what = format!("fence {logical}");
        step(context, &mut progress.steps, &context.source.name, what, fence).await?;
        println!("  fenced {}:{}", context.source.name, logical);
    }

    // Every state of the profile is its own file on the source and its own
    // file on the destination, so the reads run at once on the fenced source
    // and the writes at once on the destination: a profile with several
    // stores no longer stands the sum of every transfer over SSH.
    let reads = context
        .profile
        .state
        .iter()
        .map(|state| {
            let read = read_state(&context.source, state, &context.transaction.id, runner);
            (format!("read {}", state_location(state)), read)
        })
        .collect();
    let snapshots =
        steps_together(context, &mut progress.steps, &context.source.name, reads).await?;
    progress
        .destination_written
        .extend(snapshots.iter().map(|snapshot| snapshot.spec.clone()));
    super::journal::keep(&context.transaction.id, progress);
    let writes = snapshots
        .iter()
        .map(|snapshot| {
            let write = write_state(&context.destination, snapshot, &context.transaction.id, runner);
            (format!("write {}", state_location(&snapshot.spec)), write)
        })
        .collect();
    steps_together(context, &mut progress.steps, &context.destination.name, writes).await?;
    for snapshot in &snapshots {
        println!("  transferred {}", state_location(&snapshot.spec));
    }

    progress.route_applied = true;
    super::journal::keep(&context.transaction.id, progress);
    let routes = apply_routes(context, &context.destination.name, runner);
    step(context, &mut progress.steps, &context.destination.name, "apply routes", routes).await?;
    progress.destination_started = true;
    super::journal::keep(&context.transaction.id, progress);
    for logical in &context.profile.start_order {
        let spec = unit(destination_profile, logical)?;
        let start = act_on_unit(&context.destination, spec, UnitAction::Start, runner);
        let what = format!("start {logical}");
        step(context, &mut progress.steps, &context.destination.name, what, start).await?;
        println!("  started {}:{}", context.destination.name, logical);
    }
    // Each probe is already its own wait on its unit answering 2xx, so all of
    // them are asked at once once every unit has started; their times are
    // kept beside the steps in the profile's order, and the first refusal in
    // that order is the answer once every probe has ended.
    let mut probes = Vec::with_capacity(destination_profile.probes.len());
    for probe in &destination_profile.probes {
        let spec = unit(destination_profile, &probe.service)?;
        probes.push(async move {
            let started = Instant::now();
            let healthy = await_healthy(&context.destination, spec, &probe.url, runner).await;
            (probe, started.elapsed().as_secs_f64(), healthy)
        });
    }
    let mut refusal = None;
    for (probe, seconds, healthy) in futures::future::join_all(probes).await {
        progress.steps.push(StepTiming {
            step: format!("probe {}", probe.service),
            host: context.destination.name.clone(),
            seconds,
            succeeded: healthy.is_ok(),
        });
        match healthy {
            Ok(_) => println!("  healthy {}:{}", context.destination.name, probe.service),
            Err(error) => {
                if refusal.is_none() {
                    refusal = Some(error);
                }
            }
        }
    }
    if let Some(error) = refusal {
        return Err(error);
    }

    progress.source_retired = true;
    super::journal::keep(&context.transaction.id, progress);
    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        let retire = act_on_unit(&context.source, spec, UnitAction::Retire, runner);
        let what = format!("retire {logical}");
        step(context, &mut progress.steps, &context.source.name, what, retire).await?;
    }

    let committed = committer(prepare_committed_document(context)?, context.claim_generation.clone());
    step(context, &mut progress.steps, "the registry", "commit the placement", committed).await
}
