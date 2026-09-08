//! The ordered steps of one transaction, and the reverse of every step that
//! already happened: fence, transfer, route, start, probe, retire, commit —
//! and on any failure before the commit, restore.

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use super::readiness::{apply_routes, health_probe, preflight};
use super::state::{read_state, restore_state, write_state};
use super::units::{act_on_unit, UnitAction};
use super::{MoveContext, Progress, RegistryCommitter};
use crate::cli::placement::candidates::{deploy_error, managed_unit, profile_host, unit};
use crate::cli::{registry, CmdError};
use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::Runner;
use crate::placement::{self, PlacementUnit};
use crate::targets::ComputeTarget;

pub(in crate::cli::placement) async fn release_claim(transaction_id: &str) -> Result<(), CmdError> {
    let mut last_error = None;
    for _ in 0..3 {
        let (mut document, generation) = registry::fetch_versioned_document().await?;
        if !placement::release_transaction(&mut document, transaction_id)
            .map_err(CmdError::click)?
        {
            return Ok(());
        }
        match registry::push_document_if(&document, &generation).await {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| CmdError::click("could not release placement transaction")))
}

fn destination_record(
    destination: &ComputeTarget,
    spec: &PlacementUnit,
    managed_since: &str,
) -> Result<ManagedService, CmdError> {
    let unit = managed_unit(spec)?;
    let mut managed = if unit.kind == "launchd" {
        service::launchd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    } else {
        service::systemd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        )
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
    crate::service_resolution::retarget_profile(
        &mut document,
        &context.profile.name,
        &context.destination.name,
    )
    .map_err(CmdError::click)?;
    if !placement::release_transaction(&mut document, &context.transaction.id)
        .map_err(CmdError::click)?
    {
        return Err(CmdError::click(
            "placement transaction disappeared before commit",
        ));
    }
    Ok(document)
}

pub(in crate::cli::placement) async fn rollback(
    context: &MoveContext,
    progress: &Progress,
    runner: &Runner,
) -> Vec<String> {
    let mut errors = Vec::new();
    let destination_profile = match profile_host(&context.profile, &context.destination.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };
    let source_profile = match profile_host(&context.profile, &context.source.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };

    if progress.destination_started {
        for logical in &context.profile.stop_order {
            match unit(destination_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.destination, spec, UnitAction::Retire, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    for path in progress.destination_written.iter().rev() {
        if let Err(error) =
            restore_state(&context.destination, path, &context.transaction.id, runner).await
        {
            errors.push(error.to_string());
        }
    }
    if progress.route_applied {
        if let Err(error) = apply_routes(context, &context.source.name, runner).await {
            errors.push(error.to_string());
        }
    }
    if progress.source_stopped || progress.source_retired {
        for logical in &context.profile.start_order {
            match unit(source_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.source, spec, UnitAction::Start, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    errors
}

pub(in crate::cli::placement) async fn execute_move(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
    committer: &RegistryCommitter,
) -> Result<String, CmdError> {
    preflight(context, runner).await?;
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    println!(
        "moving {}: {} -> {}",
        context.profile.name, context.source.name, context.destination.name
    );
    progress.source_stopped = true;
    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        act_on_unit(&context.source, spec, UnitAction::Stop, runner).await?;
        println!("  fenced {}:{}", context.source.name, logical);
    }

    let mut snapshots = Vec::with_capacity(context.profile.state.len());
    for state in &context.profile.state {
        snapshots.push(read_state(&context.source, state, runner).await?);
    }
    for snapshot in &snapshots {
        progress
            .destination_written
            .push(snapshot.spec.path.clone());
        write_state(
            &context.destination,
            snapshot,
            &context.transaction.id,
            runner,
        )
        .await?;
        println!("  transferred $HOME/{}", snapshot.spec.path);
    }

    progress.route_applied = true;
    apply_routes(context, &context.destination.name, runner).await?;
    progress.destination_started = true;
    for logical in &context.profile.start_order {
        let spec = unit(destination_profile, logical)?;
        act_on_unit(&context.destination, spec, UnitAction::Start, runner).await?;
        println!("  started {}:{}", context.destination.name, logical);
    }
    for probe in &destination_profile.probes {
        health_probe(&context.destination, &probe.url, 30, runner).await?;
        println!("  healthy {}:{}", context.destination.name, probe.service);
    }

    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        act_on_unit(&context.source, spec, UnitAction::Retire, runner).await?;
    }
    progress.source_retired = true;

    let committed = prepare_committed_document(context)?;
    committer(committed, context.claim_generation.clone()).await
}
