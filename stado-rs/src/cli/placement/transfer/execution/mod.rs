//! The ordered steps of one move: preflight and a copy of every state before
//! the transaction is claimed ([`prepare_move`]), then under the claim fence,
//! copy what changed, install, route, start, probe, retire, commit. A
//! profile carrying the fleet vault holds the vault's writes instead of
//! stopping it and retires it after the commit ([`retire_after_commit`]).
//! The reverse of every step that already happened, run on any failure
//! before the commit, is [`rollback`].

use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use super::cutover::promote_standby;
use super::readiness::{apply_routes, await_healthy, preflight, stop_empty_copies};
use super::state::{discard_staging, read_state, state_location, write_state};
use super::units::{act_on_unit, UnitAction};
use super::{MoveContext, Progress, RegistryCommitter, StepTiming};
use crate::cli::placement::candidates::{deploy_error, managed_unit, profile_host, unit};
use crate::cli::CmdError;
use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::Runner;
use crate::placement::{self, PlacementUnit};
use crate::targets::ComputeTarget;

mod rollback;
mod timed;

pub(in crate::cli::placement) use rollback::{release_claim, rollback};
use timed::{step, steps_together};

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

/// What a move does before it claims its transaction, while the source
/// still serves every consumer: the preflight, then a copy of every state
/// into the staging copy the read after the fence fills again by content.
/// An open transaction refuses the profile's services to every consumer, so
/// a copy made under the claim would hold them down for the whole copy; made
/// here, they are down for what changed in between. The journal is kept from
/// the start, so a `placement transactions` sweep keeps the copies of a
/// move whose process still runs.
pub(in crate::cli::placement) async fn prepare_move(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
) -> Result<(), CmdError> {
    super::journal::keep(&context.transaction.id, progress);
    let both = format!("{} and {}", context.source.name, context.destination.name);
    let checked = preflight(context, runner);
    step(context, &mut progress.steps, &both, "preflight", checked).await?;
    let copies = context
        .profile
        .state
        .iter()
        .map(|state| {
            let copy = read_state(&context.source, state, &context.transaction.id, runner);
            (format!("copy {} while it serves", state_location(state)), copy)
        })
        .collect();
    steps_together(context, &mut progress.steps, &context.source.name, copies).await?;
    Ok(())
}

/// The claimed half of a move [`prepare_move`] prepared: fence, copy what
/// changed, install, route, start, probe, retire, commit.
pub(in crate::cli::placement) async fn execute_move(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
    committer: &RegistryCommitter,
) -> Result<String, CmdError> {
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    println!(
        "moving {}: {} -> {}",
        context.profile.name, context.source.name, context.destination.name
    );
    let emptied = stop_empty_copies(context, runner);
    for logical in step(
        context,
        &mut progress.steps,
        &context.destination.name,
        "stop empty copies",
        emptied,
    )
    .await?
    {
        println!(
            "  stopped the empty copy {}:{}",
            context.destination.name, logical
        );
    }
    // Every step counts as taken, in memory and in the journal on this host,
    // before it starts: a rollback or a later release then undoes a step that
    // was cut short, and never one that did not begin.
    let write_fenced = super::vault_fence::write_fenced(&context.profile);
    if write_fenced {
        // The fleet vault keeps answering reads, the registry authority's
        // among them, until the commit; only its writes are held.
        progress.source_write_fenced = true;
        super::journal::keep(&context.transaction.id, progress);
        let fence = super::vault_fence::arm(context, runner);
        step(
            context,
            &mut progress.steps,
            &context.source.name,
            "hold the vault's writes",
            fence,
        )
        .await?;
        println!(
            "  {}: the vault holds its writes for {} and keeps answering reads",
            context.source.name, context.transaction.id
        );
    } else {
        progress.source_stopped = true;
        super::journal::keep(&context.transaction.id, progress);
        for logical in &context.profile.stop_order {
            let spec = unit(source_profile, logical)?;
            let fence = act_on_unit(&context.source, spec, UnitAction::Stop, runner);
            let what = format!("fence {logical}");
            step(
                context,
                &mut progress.steps,
                &context.source.name,
                what,
                fence,
            )
            .await?;
            println!("  fenced {}:{}", context.source.name, logical);
        }
    }

    // Every state is its own file or tree on the source and on the
    // destination, so the reads run at once on the fenced source and the
    // writes at once on the destination: a profile with several stores does
    // not stand the sum of every transfer over SSH.
    let reads = context
        .profile
        .state
        .iter()
        .map(|state| {
            let read = read_state(&context.source, state, &context.transaction.id, runner);
            (format!("read what changed in {}", state_location(state)), read)
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
            let write = write_state(
                &context.destination,
                snapshot,
                &context.transaction.id,
                runner,
            );
            (format!("write {}", state_location(&snapshot.spec)), write)
        })
        .collect();
    steps_together(
        context,
        &mut progress.steps,
        &context.destination.name,
        writes,
    )
    .await?;
    for snapshot in &snapshots {
        println!("  transferred {}", state_location(&snapshot.spec));
    }
    discard_staging(&context.transaction.id)?;

    progress.route_applied = true;
    super::journal::keep(&context.transaction.id, progress);
    let routes = apply_routes(context, &context.destination.name, runner);
    step(
        context,
        &mut progress.steps,
        &context.destination.name,
        "apply routes",
        routes,
    )
    .await?;
    progress.destination_started = true;
    super::journal::keep(&context.transaction.id, progress);
    for logical in &context.profile.start_order {
        let spec = unit(destination_profile, logical)?;
        let start = act_on_unit(&context.destination, spec, UnitAction::Start, runner);
        let what = format!("start {logical}");
        step(
            context,
            &mut progress.steps,
            &context.destination.name,
            what,
            start,
        )
        .await?;
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

    // A write-fenced source keeps serving reads through the commit, the
    // registry authority's own among them, and is retired after it
    // ([`retire_after_commit`]).
    if !write_fenced {
        progress.source_retired = true;
        super::journal::keep(&context.transaction.id, progress);
        for logical in &context.profile.stop_order {
            let spec = unit(source_profile, logical)?;
            let retire = act_on_unit(&context.source, spec, UnitAction::Retire, runner);
            let what = format!("retire {logical}");
            step(
                context,
                &mut progress.steps,
                &context.source.name,
                what,
                retire,
            )
            .await?;
        }
    } else {
        // The commit sends the source's registry authority and object API to
        // the destination's vault at once, so the source reads it first over
        // the same channel ([`super::vault_reach`]); a refusal here rolls the
        // move back with nothing committed.
        let reach = super::vault_reach::source_reads_destination(context, runner);
        let what = format!(
            "read the destination's vault from {}",
            context.source.name
        );
        step(context, &mut progress.steps, &context.source.name, what, reach).await?;
    }

    let committed = committer(
        prepare_committed_document(context)?,
        context.claim_generation.clone(),
    );
    let error = match step(
        context,
        &mut progress.steps,
        "the registry",
        "commit the placement",
        committed,
    )
    .await
    {
        Ok(generation) => return Ok(generation),
        Err(error) => error,
    };
    // A compare-and-swap whose write landed can still lose its receipt: the
    // authority answers after writing, and its own answer can fail (a vault
    // move's authority loses the vault the moment the resolver follows the
    // commit). Rolled back on that, the hosts were undone under a registry
    // that already placed the profile on the destination. So the registry
    // is read back from the authority itself: this host's resolver froze at
    // the claim while the authority's store was down, and its copy, read
    // here, took a landed commit for none. This move's commit standing at the
    // authority is a commit; an authority that does not answer leaves the
    // outcome unknown, which is never rolled back.
    let read = match crate::cli::resolver::authority_document().await {
        Ok(Some(read)) => Ok(read),
        Ok(None) => crate::cli::registry::fetch_versioned_document()
            .await
            .map_err(|error| error.to_string()),
        Err(unread) => Err(unread),
    };
    match read {
        Ok((document, generation)) if landed(context, &document) => {
            println!(
                "  the registry authority holds this move's commit at generation {generation}; \
                 its receipt was lost ({error})"
            );
            Ok(generation)
        }
        Ok(_) => Err(error),
        Err(unread) => {
            progress.commit_unknown = true;
            Err(error.also(format!(
                "whether the commit landed is unknown: the registry authority could not be read \
                 back ({unread}); nothing was rolled back. Once the authority answers, `stado \
                 resolver follow` brings this host's copy to its generation and `stado placement \
                 transactions` then says it: transaction {} still open means it did not land and \
                 `stado placement release {}` undoes the move; gone means it committed",
                context.transaction.id, context.transaction.id
            )))
        }
    }
}

/// Whether `document` holds this move's commit: its transaction released
/// and every service of the profile active on the destination.
fn landed(context: &MoveContext, document: &Value) -> bool {
    let open = placement::transactions(document)
        .map(|open| open.iter().any(|held| held.id == context.transaction.id))
        .unwrap_or(true);
    !open
        && context.profile.services.iter().all(|service| {
            document["service_directory"]["services"][service]["active_host"].as_str()
                == Some(context.destination.name.as_str())
        })
}

/// The write-fenced source's units, retired once the move committed: first
/// the source host's own resolver is asked to follow the committed registry,
/// so every consumer there that reads the service through its adapter is
/// sent to the destination, then each unit is retired, then the retired
/// copy's fence is lifted. The move is committed whatever this answers, so
/// nothing here rolls back; each failure is returned, said with what stands:
/// a source unit still running keeps answering reads with every write held,
/// so a consumer still sent there reads what was moved and writes nothing
/// that would be lost.
pub(in crate::cli::placement) async fn retire_after_commit(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
) -> Vec<String> {
    let source = &context.source.name;
    let followed = crate::cli::host::remote_stado_output(source, &["resolver", "follow"]);
    if let Err(error) = step(
        context,
        &mut progress.steps,
        source,
        "the source's resolver follows the commit",
        followed,
    )
    .await
    {
        return vec![format!(
            "{source}'s resolver did not follow the committed registry ({error}), so its units \
             were not retired: they keep answering reads with every write held; retire them with \
             `stado service retire` once `stado host exec {source} -- stado resolver status \
             --json` holds the new generation"
        )];
    }
    let source_profile = match profile_host(&context.profile, source) {
        Ok(profile) => profile,
        Err(error) => return vec![error.to_string()],
    };
    let mut failures = Vec::new();
    for logical in &context.profile.stop_order {
        let retired = match unit(source_profile, logical) {
            Ok(spec) => {
                let retire = act_on_unit(&context.source, spec, UnitAction::Retire, runner);
                step(context, &mut progress.steps, source, format!("retire {logical}"), retire)
                    .await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = retired {
            failures.push(format!(
                "{source}: {logical} was not retired after the commit: {error}; it keeps \
                 answering reads with every write held"
            ));
        }
    }
    // With no unit left serving the source's copy, its fence holds nothing
    // back and would refuse every write to the vault there if the vault ever
    // moved back to that host; a unit still running keeps its fence.
    if failures.is_empty() {
        let lifted = super::vault_fence::lift(context, runner);
        if let Err(error) = step(
            context,
            &mut progress.steps,
            source,
            "lift the retired copy's fence",
            lifted,
        )
        .await
        {
            failures.push(format!(
                "{source}: the retired copy's fence was not lifted ({error}); a move of the vault \
                 back to {source} finds it standing: lift it with `skarbiec fence lift \
                 --transaction {}` there",
                context.transaction.id
            ));
        }
    }
    failures
}
