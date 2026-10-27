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

mod check;
mod deliver;
mod ports;
mod template;

use serde::Serialize;

use crate::cli::CmdError;

pub(crate) use check::check;
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

/// Prepare `host` for `profile`. See the module doc for the steps. With
/// `check`, only the refusals a pass gives before it delivers anything, and
/// what it would deliver.
pub(crate) async fn standby(
    profile_name: &str,
    host: &str,
    reason: &str,
    check_only: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if check_only {
        let checked = check(profile_name, host, reason).await?;
        if json_output {
            println!("{}", serde_json::to_string_pretty(&checked)?);
            return Ok(());
        }
        for service in checked["services"].as_array().into_iter().flatten() {
            use crate::cli::reporting::field;
            println!(
                "{host}\t{}\t{}\t{}\t{}",
                field(&service["service"]),
                field(&service["delivery"]),
                field(&service["product"]),
                field(&service["release_control"]["product"])
            );
        }
        println!("{profile_name}\t{host}\twould_stand_by (checked; nothing delivered or written)");
        return Ok(());
    }
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
    let check::Planned { document, profile, target, placed, placed_on, plan } =
        check::planned(profile_name, host, reason).await?;
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

    // Step 2: deliveries. A managed program is delivered here; a release-
    // controlled tree is declared for the host's agent to roll out.
    let mut awaiting = Vec::new();
    // Every step below is a wait naming the profile, the host and the step,
    // so a pass that stands for minutes names what it stands on in `stado
    // waits` and a later pass's wait on it opens with when it is due. A
    // managed program is a release delivered over the host's own channel, so
    // those run at once; a release-controlled tree is a compare-and-swapped
    // registry commit, so those run one after another and never race each
    // other's generation. The report keeps the plan's order, and the first
    // refusal in that order is the answer.
    let waited = |what: String| format!("placement standby of {} on {}: {what}", profile.name, target.name);
    let (programs, trees): (Vec<_>, Vec<_>) = plan.services.iter().enumerate().partition(
        |(_, service)| matches!(service.kind, template::ProgramKind::Managed { .. }),
    );
    let mut answers: Vec<Option<Result<deliver::Delivery, CmdError>>> =
        plan.services.iter().map(|_| None).collect();
    let shared = (&document, &placed, &target);
    let delivered = programs.into_iter().map(|(index, service)| {
        let what = waited(format!("deliver {}", service.logical));
        let (document, placed, target) = shared;
        async move { (index, deliver::deliver(document, placed, target, service, what).await) }
    });
    for (index, answer) in futures::future::join_all(delivered).await {
        answers[index] = Some(answer);
    }
    for (index, service) in trees {
        let what = waited(format!("deliver {}", service.logical));
        answers[index] = Some(deliver::deliver(&document, &placed, &target, service, what).await);
    }
    for answer in answers.into_iter().flatten() {
        let delivery = answer?;
        if delivery.outcome == deliver::words::ROLLOUT_DECLARED
            || delivery.outcome == deliver::words::ROLLOUT_PENDING
        {
            awaiting.push(delivery.service.clone());
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
            listen_port: ports::standby_port(&document, logical, &target.name, &placed_on)?,
        };
        let staged =
            crate::cli::service::lifecycle::declare::ensure::run::stage_unit(options, standby);
        let staged = crate::wait::until(
            crate::wait::Kind::Host,
            waited(format!("stage unit {logical}")),
            &target.name,
            staged,
        );
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
    let generation = crate::wait::until(
        crate::wait::Kind::Host,
        waited("write the host into the profile and the directory".to_string()),
        "the registry",
        template::commit_host(&profile, &placed_on, &target, &report.units),
    )
    .await?;
    report.detail = format!(
        "{} may now move to {} (registry generation {generation})",
        profile.name, target.name
    );
    report.registry_generation = Some(generation);
    report.outcome = words::DECLARED.to_string();
    Ok(report)
}
