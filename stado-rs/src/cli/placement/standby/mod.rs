//! `stado placement standby PROFILE --host HOST` — make a registered host a
//! place a placement profile may move to.
//!
//! A profile declares the hosts it may run on, and that list used to grow
//! only by hand: an operator delivered each product to the host, asserted
//! each unit, and edited the profile and the directory. A profile can
//! declare a small control host and a laptop while a workstation with many
//! times the memory sits in the same registry, undeclared, because nobody
//! typed the six commands. `stado placement move` moves a profile only
//! between declared hosts, so the fleet's largest machine is not a place it
//! can move to.
//!
//! This command is those steps as one idempotent pass, each step named in
//! the receipt and refused by name:
//!
//! 1. the profile's services are resolved through the service catalog, and
//!    each is classified by what its program is: a managed program under
//!    `$HOME/.stado/bin` (delivered through the same manifest-verified path
//!    `stado release version converge` uses, at the version the host or
//!    the placed host declares) or a release-controlled tree under
//!    `$HOME/.stado/services/<product>/current` (rolled out by the host's
//!    own agent once the product's release control names the host);
//! 2. the deliveries are declared and, for managed programs, performed;
//! 3. every unit is rendered by the same pass `service ensure` runs, from
//!    the catalog for the host's platform, and staged: written where the
//!    move starts it, left unloaded, and not declared on the host. A move
//!    refuses a destination that already runs the service, and its commit
//!    is what carries the declaration from the placed host, so a declared
//!    or running unit on a standby host is a second active copy;
//! 4. only when every unit is staged is the host written into the profile,
//!    with the units it now holds and the placed host's probes, and into the
//!    directory as a standby address for each service the directory does not
//!    already name it under — a host with some of a profile's units is a
//!    split profile no move accepts, so it is never written half.
//!
//! A pass that stops early (a tree not yet rolled out) reports where it
//! stopped; the next pass continues from there.

mod deliver;
mod template;

use serde::Serialize;

use crate::cli::{registry, CmdError};
use crate::placement;

pub use deliver::Delivery;

/// The words a standby pass ends with.
pub mod words {
    /// A release-controlled tree is declared for the host but its agent has
    /// not rolled it out yet; the units that depend on it were not staged.
    pub const AWAITING_RELEASE_ROLLOUT: &str = "awaiting_release_rollout";
    /// Every unit is staged and the host is written into the profile and
    /// the directory.
    pub const DECLARED: &str = "declared";
}

#[derive(Debug, Clone, Serialize)]
pub struct StandbyReport {
    pub profile: String,
    pub host: String,
    pub placed_on: String,
    pub platform: String,
    /// Why the operator asked this host to stand by.
    pub reason: String,
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
    /// The unit file staged on the host.
    pub path: String,
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
            "{}\t{}\t{}\t{}\t{}",
            report.host, unit.service, unit.label, unit.action, unit.path
        );
    }
    println!(
        "{}\t{}\t{}\t{}",
        report.profile, report.host, report.outcome, report.detail
    );
    Ok(())
}

/// The pass behind the command.
pub(crate) async fn prepare(
    profile_name: &str,
    host: &str,
    reason: &str,
) -> Result<StandbyReport, CmdError> {
    if reason.trim().is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host must stand by for the profile; the receipt carries \
             it",
        ));
    }
    let (document, _generation) = registry::fetch_versioned_document().await?;
    let registry = super::candidates::parse_registry(&document)?;
    let profile = placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the registry declares no placement profile named {profile_name:?}"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    super::candidates::ensure_profile_lifecycle_mutable(&profile)?;
    let target = crate::cli::canonical_host(host).await?;
    let placed_on = super::placed_host(&registry, &profile).map_err(CmdError::declaration)?;
    if placed_on == target.name {
        return Err(CmdError::refused(format!(
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
        reason: reason.to_string(),
        outcome: String::new(),
        deliveries: Vec::new(),
        units: Vec::new(),
        registry_generation: None,
        detail: String::new(),
    };

    let plan = template::plan(&profile, &placed, &target)?;

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

    // Step 3: units, in the profile's start order, staged: written where the
    // move starts them and left unloaded and undeclared, because the move
    // refuses a destination that already runs the service and carries the
    // declaration over itself. A unit whose tree is still rolling out is not
    // staged: it would render a program that is not on the host yet.
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
            unset_env: &[],
            reason,
            as_daemon: service.as_daemon,
            as_launch_agent: false,
            as_json: true,
        };
        let standby = crate::cli::service::lifecycle::declare::ensure::Standby {
            listen_port: template::standby_port(&document, logical, &target.name, &placed_on)?,
        };
        let staged =
            crate::cli::service::lifecycle::declare::ensure::run::stage_unit(options, standby);
        let receipt = match staged.await {
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
            path: receipt.path,
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

    // Step 4: the host into the profile with the units it now holds, and into
    // the directory, in one compare-and-swapped commit.
    let generation = template::commit_host(&profile, &placed_on, &target, &report.units).await?;
    report.detail = format!(
        "{} may now move to {} (registry generation {generation})",
        profile.name, target.name
    );
    report.registry_generation = Some(generation);
    report.outcome = words::DECLARED.to_string();
    Ok(report)
}
