//! `stado placement standby PROFILE --host HOST` — make a registered host a
//! place a placement profile may move to.
//!
//! A profile declares the hosts it may run on, and until 2026-09-18 that
//! list grew only by hand: an operator delivered each product to the host,
//! asserted each unit, and edited the profile and the directory. The
//! `brama-skarbiec` profile declared the 16 GiB control host and a laptop
//! while a workstation with 123 GiB sat in the same registry, undeclared,
//! because nobody had typed the six commands. Placement relief moves a
//! profile only between declared hosts, so the fleet's largest machine was
//! not a place it could move to.
//!
//! This command is those six steps as one idempotent pass, each step named
//! in the receipt and refused by name, so the autonomy cycle can run it too
//! (placement relief prepares a standby when no declared host has headroom):
//!
//! 1. the profile's services are resolved through the service catalog, and
//!    each is classified by what its program is: a managed program under
//!    `$HOME/.stado/bin` (delivered through the same manifest-verified path
//!    `stado release host-state --apply` uses, at the version the host or
//!    the placed host declares) or a release-controlled tree under
//!    `$HOME/.stado/services/<product>/current` (rolled out by the host's
//!    own agent once the product's release control names the host);
//! 2. the deliveries are declared and, for managed programs, performed;
//! 3. every unit is asserted through the same `service ensure` an operator
//!    runs, rendered from the catalog for the host's platform;
//! 4. only when every unit is declared on the host is the host written into
//!    the profile, with the units the registry now declares and the placed
//!    host's probes, and into the directory as a standby endpoint for each
//!    service — a host with some of a profile's units is a split profile no
//!    move accepts, so it is never written half.
//!
//! A pass that stops early (a tree not yet rolled out) reports where it
//! stopped; the next pass continues from there.

mod deliver;
mod template;

use serde::Serialize;

use crate::cli::{registry, CmdError};
use crate::deploy::host_channel;
use crate::placement;

pub use deliver::Delivery;

/// The words a standby pass ends with.
pub mod words {
    /// The host already declares every unit of the profile and sits in it.
    pub const ALREADY_DECLARED: &str = "already_declared";
    /// A release-controlled tree is declared for the host but its agent has
    /// not rolled it out yet; the units that depend on it were not asserted.
    pub const AWAITING_RELEASE_ROLLOUT: &str = "awaiting_release_rollout";
    /// Every unit is asserted and the host is written into the profile and
    /// the directory.
    pub const DECLARED: &str = "declared";
}

#[derive(Debug, Clone, Serialize)]
pub struct StandbyReport {
    pub profile: String,
    pub host: String,
    pub placed_on: String,
    pub platform: String,
    pub outcome: String,
    pub deliveries: Vec<Delivery>,
    pub units: Vec<UnitReceipt>,
    pub registry_generation: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitReceipt {
    pub service: String,
    pub label: String,
    pub action: String,
    pub pid: Option<u32>,
}

/// Prepare `host` for `profile`. See the module doc for the steps.
pub(crate) async fn standby(
    profile_name: &str,
    host: &str,
    reason: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = prepare(profile_name, host, reason).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for delivery in &report.deliveries {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            report.host, delivery.service, delivery.kind, delivery.outcome, delivery.detail
        );
    }
    for unit in &report.units {
        println!(
            "{}\t{}\t{}\t{}\tpid {}",
            report.host,
            unit.service,
            unit.label,
            unit.action,
            unit.pid
                .map_or_else(|| "-".to_string(), |pid| pid.to_string())
        );
    }
    println!(
        "{}\t{}\t{}\t{}",
        report.profile, report.host, report.outcome, report.detail
    );
    Ok(())
}

/// The pass, shared by the command and by placement relief.
pub(crate) async fn prepare(
    profile_name: &str,
    host: &str,
    reason: &str,
) -> Result<StandbyReport, CmdError> {
    if reason.trim().is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host must stand by for the profile; it is recorded \
             beside every unit the pass declares",
        ));
    }
    let (document, _generation) = registry::fetch_versioned_document().await?;
    let registry = super::candidates::parse_registry(&document)?;
    let profile = placement::profiles(&document)
        .map_err(CmdError::click)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the registry declares no placement profile named {profile_name:?}"
            ))
        })?;
    super::candidates::ensure_profile_lifecycle_mutable(&profile)?;
    let target = host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let placed_on = super::placed_host(&registry, &profile).map_err(CmdError::click)?;
    if placed_on == target.name {
        return Err(CmdError::click(format!(
            "{} is placed on {placed_on}; a placed host cannot stand by for its own profile",
            profile.name
        )));
    }
    let placed = super::candidates::target(&registry, &placed_on)?.clone();
    let mut report = StandbyReport {
        profile: profile.name.clone(),
        host: target.name.clone(),
        placed_on: placed_on.clone(),
        platform: target.release_platform.clone(),
        outcome: String::new(),
        deliveries: Vec::new(),
        units: Vec::new(),
        registry_generation: None,
        detail: String::new(),
    };

    let plan = template::plan(&profile, &placed, &target)?;
    if template::already_declared(&profile, &target, &plan) {
        report.outcome = words::ALREADY_DECLARED.to_string();
        report.detail = format!(
            "{} declares every unit of {} and sits in the profile",
            target.name, profile.name
        );
        return Ok(report);
    }

    // Step 2: deliveries. A managed program is delivered here; a release-
    // controlled tree is declared for the host's agent to roll out.
    let mut awaiting = Vec::new();
    for service in &plan.services {
        let delivery = deliver::deliver(&document, &placed, &target, service).await?;
        if delivery.outcome == deliver::words::ROLLOUT_DECLARED
            || delivery.outcome == deliver::words::ROLLOUT_PENDING
        {
            awaiting.push(service.logical.clone());
        }
        report.deliveries.push(delivery);
    }

    // Step 3: units, in the profile's start order. A unit whose tree is
    // still rolling out is not asserted: `ensure` would render a program
    // that is not on the host yet and fail the postcondition.
    for logical in &profile.start_order {
        let Some(service) = plan.services.iter().find(|s| &s.logical == logical) else {
            continue;
        };
        if awaiting.contains(logical) {
            continue;
        }
        let options = crate::cli::service::EnsureOptions {
            name: &service.catalog_name,
            host: &target.name,
            from: None,
            args: &[],
            env: &[],
            reason,
            as_daemon: service.as_daemon,
            as_launch_agent: false,
            as_json: true,
        };
        let receipt = match crate::cli::service::ensure_unit(options).await {
            Ok(receipt) => receipt,
            // A release-controlled tree the agent has not staged yet renders
            // a program that is not on the host: that is the rollout still
            // pending, not a refusal of the pass.
            Err(error) if matches!(service.kind, template::ProgramKind::Tree { .. }) => {
                awaiting.push(logical.clone());
                if let Some(delivery) = report
                    .deliveries
                    .iter_mut()
                    .find(|delivery| &delivery.service == logical)
                {
                    delivery.outcome = deliver::words::ROLLOUT_PENDING.to_string();
                    delivery.detail = error.to_string();
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        report.units.push(UnitReceipt {
            service: logical.clone(),
            label: receipt.label,
            action: receipt.action,
            pid: receipt.pid,
        });
    }
    if !awaiting.is_empty() {
        report.outcome = words::AWAITING_RELEASE_ROLLOUT.to_string();
        report.detail = format!(
            "{} is declared for {} on {}; the host's agent rolls the release out, and the next \
             pass asserts the unit and writes the host into the profile",
            awaiting.join(", "),
            profile.name,
            target.name
        );
        return Ok(report);
    }

    // Step 4: the host into the profile and the directory, from what the
    // registry now declares on it, in one compare-and-swapped commit.
    let generation = template::commit_host(&profile, &placed_on, &target, &plan).await?;
    report.detail = format!(
        "{} may now move to {} (registry generation {generation})",
        profile.name, target.name
    );
    report.registry_generation = Some(generation);
    report.outcome = words::DECLARED.to_string();
    Ok(report)
}
