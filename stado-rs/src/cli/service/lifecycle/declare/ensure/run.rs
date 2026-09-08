//! `service ensure`: the host half of one pass.

use super::*;

/// `service ensure NAME --host HOST [--from PATH] --reason WHY`.
///
/// The idempotent half of `deploy`, and the only one that works on an ssh
/// login with no Aqua session. Two facts decide everything it does, and both
/// come from the host: what the unit on the box declares it runs, and what the
/// process under it is actually running. See
/// [`crate::deploy::service::ensure_service`].
pub(crate) async fn ensure(options: EnsureOptions<'_>) -> Result<(), CmdError> {
    let reason = options.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host has to run this unit; it is recorded beside the \
             registry document this command declares the unit in",
        ));
    }
    let target = host_channel::canonical_target(options.host)
        .await
        .map_err(click)?;
    let host = target.name.clone();
    if options.as_launch_agent && !target.release_platform.starts_with("darwin") {
        return Err(CmdError::click("--as-launch-agent is Darwin-only"));
    }

    // Resolve the operator-facing name against both declarations that may
    // supply a stable init-system identity. The service catalog owns authored
    // services; the managed-product catalog owns units that execute a delivered
    // product binary.
    let declared = service::declared_services(&target);
    let catalog_entry = crate::deploy::service_catalog::lookup(options.name)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let catalog_unit = catalog_entry.as_ref().and_then(|entry| entry.unit.clone());
    let managed_unit = canonical_managed_unit(options.name, &target.name)?;
    let canonical_unit = match (catalog_unit, managed_unit) {
        (Some(service_unit), Some(product_unit)) if service_unit != product_unit => {
            return Err(CmdError::click(format!(
                "service and managed-product declarations disagree about {}: {} versus {}",
                options.name, service_unit, product_unit
            )));
        }
        (Some(unit), _) | (_, Some(unit)) => Some(unit),
        (None, None) => None,
    };
    let existing = declared.iter().find(|candidate| {
        candidate.matches(options.name)
            || canonical_unit
                .as_deref()
                .is_some_and(|unit| candidate.matches(unit))
    });
    // The catalog's environment is the product's own requirement for the
    // unit, so it applies whatever declared the program: a registry entry
    // adopted from a hand-installed plist names the same binary and still
    // needs the same variables. Program and args keep their resolution
    // order; only the environment is defaulted from the catalog.
    let mut unit_env: Vec<(String, String)> = catalog_entry
        .as_ref()
        .map(|entry| {
            crate::deploy::service_catalog::resolve_entry(
                entry,
                &crate::deploy::service_catalog::home_for(&target),
                Some(&target.release_platform),
                &target.name,
            )
            .2
        })
        .unwrap_or_default();
    let mut unit = unit_program(&host, options.name, options.from, options.args, existing)?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: options.name.to_string(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
            env: unit.env.clone(),
            repair: Vec::new(),
        };
        let (program, args, env) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &crate::deploy::service_catalog::home_for(&target),
            Some(&target.release_platform),
            &target.name,
        );
        unit.program = program;
        unit.args = args;
        unit_env = env;
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the Wisent service \
             catalog this build ships: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    if unit.source == "shipped" {
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the declaration shipped \
             with this build: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    let home = crate::deploy::service_catalog::home_for(&target);
    let mut env_overrides = unit.env;
    for assignment in options.env {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| CmdError::usage("--env requires NAME=VALUE"))?;
        env_overrides.insert(name.to_string(), value.to_string());
    }
    for (name, value) in env_overrides {
        let value = crate::deploy::service_catalog::resolve_word(
            &value,
            &home,
            Some(&target.release_platform),
            &target.name,
        );
        match unit_env.iter_mut().find(|(key, _)| key == &name) {
            Some((_, current)) => *current = value,
            None => unit_env.push((name, value)),
        }
    }
    // A canonical declaration wins, then the identity carried by the resolved
    // program, then the unit already declared on this host. The canonical
    // identity must win even when the registry supplies the program: otherwise
    // a stale doubled unit name is faithfully re-rendered forever.
    let plan = match canonical_unit
        .as_deref()
        .or(unit.unit.as_deref())
        .or_else(|| existing.and_then(declared_label))
    {
        Some(label) => service::plan_deploy_labelled(
            &target,
            options.name,
            label,
            &unit.program,
            &unit.args,
            &unit_env,
        ),
        None => service::plan_deploy_labelled(
            &target,
            options.name,
            &crate::deploy::local_install::label(service::DEPLOY_KIND, options.name),
            &unit.program,
            &unit.args,
            &unit_env,
        ),
    }
    .map_err(click)?;
    let mut plan = plan;
    if !unit.systemd_unit.is_empty() {
        if !target.release_platform.starts_with("linux") {
            return Err(CmdError::click(format!(
                "{} declares a systemd unit definition on non-Linux platform {}",
                options.name, target.release_platform
            )));
        }
        let definition = std::mem::take(&mut unit.systemd_unit);
        unit.systemd_unit =
            service::retain_systemd_unit(&mut plan, &definition, &unit_env, unit.source == "flag")
                .map_err(click)?;
    }
    // A declared path is the service's durable domain choice. In particular,
    // a LaunchAgent intentionally placed on an always-on Mac must not become
    // a daemon again when ensure or the autonomy reconciler runs later.
    if options.as_launch_agent
        || (existing
            .is_some_and(|declared| service::UnitDomain::from_path(&declared.path).is_per_login())
            && !options.as_daemon)
    {
        plan.force_daemon = false;
    } else {
        // The target default remains the safe answer for undeclared services,
        // and --as-daemon can still turn the system domain on explicitly.
        plan.force_daemon = plan.force_daemon || options.as_daemon;
    }

    // An existing declaration is not a refusal here, and that is the whole
    // difference from `deploy`: asserting a unit that is already declared and
    // already running is what makes this safe to run twice, or from a script.
    let already = declared.into_iter().find(|candidate| {
        candidate.matches(options.name)
            || candidate.matches(&plan.label)
            || candidate.matches(&plan.unit)
    });

    let runner = production_runner();
    let outcome = service::ensure_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !outcome.succeeded() {
        let mut detail = format!(
            "{host}: could not ensure {}: {}",
            options.name,
            outcome.report.failure()
        );
        if !outcome.report.postcondition_held() {
            // A unit that will not stay up on a host where the same program
            // already runs outside any unit is the four-day incident from the
            // other side: launchd is being asked for a port a disowned process
            // still holds.
            detail.push_str(
                ". `stado service list --unowned` names a process that may still hold its port",
            );
        }
        return Err(CmdError::click(detail));
    }

    let mut record = service::record_from_ensure(&host, options.name, &outcome, &now());
    record.program = unit.program;
    record.args = unit.args;
    record.env = unit_env.into_iter().collect();
    record.systemd_unit = unit.systemd_unit;
    let persisted = persist_ensure_record(&record, &already, &outcome, &plan, reason, &host).await;
    let audited = persisted.map_err(|mut error| {
        let cause = error
            .message
            .as_deref()
            .unwrap_or("registry operation failed");
        error.failure = Some(
            error
                .failure
                .unwrap_or_else(|| crate::failure::classify_message(cause)),
        );
        error.message = Some(format!(
            "{host}: {} is running (action {}, pid {}), but recording the completed ensure \
             failed: {cause}. No host action was repeated.",
            options.name,
            outcome.action,
            outcome.pid.trim(),
        ));
        error.json = options.as_json;
        error
    })?;

    if options.as_json {
        // Exactly the contract's keys: a desktop client consumes this shape.
        // Where the record landed goes to stderr rather than into the object.
        if let Some(audited) = audited.as_deref() {
            eprintln!("audit record {audited}");
        }
        return print_json(&json!({
            "host": host,
            "name": record.name,
            "label": record.unit_id(),
            "domain": outcome.domain_word(),
            "action": outcome.action,
            "pid": outcome.pid.trim().parse::<u32>().ok(),
        }));
    }
    table::print(
        &["HOST", "SERVICE", "LABEL", "DOMAIN", "ACTION", "PID"],
        &[vec![
            host,
            record.name.clone(),
            record.unit_id().to_string(),
            outcome.domain_word().to_string(),
            outcome.action.clone(),
            dash(outcome.pid.trim()),
        ]],
    );
    if let Some(audited) = audited.as_deref() {
        println!("audit record {audited}");
    }
    Ok(())
}
