//! `service ensure`: the host half of one pass.

use super::*;

/// What one `ensure` pass left on the host and in the registry.
pub(crate) struct EnsureReceipt {
    pub(crate) host: String,
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) domain: String,
    pub(crate) action: String,
    pub(crate) pid: Option<u32>,
    pub(crate) audited: Option<String>,
}

/// `service ensure NAME --host HOST [--from PATH] --reason WHY`.
///
/// The idempotent half of `deploy`, and the only one that works on an ssh
/// login with no Aqua session. Two facts decide everything it does, and both
/// come from the host: what the unit on the box declares it runs, and what the
/// process under it is actually running. See
/// [`crate::deploy::service::ensure_service`].
pub(crate) async fn ensure(options: EnsureOptions<'_>) -> Result<(), CmdError> {
    let as_json = options.as_json;
    let receipt = ensure_unit(options).await?;
    if as_json {
        // Exactly the contract's keys: a desktop client consumes this shape.
        // Where the record landed goes to stderr rather than into the object.
        if let Some(audited) = receipt.audited.as_deref() {
            eprintln!("audit record {audited}");
        }
        return print_json(&json!({
            "host": receipt.host,
            "name": receipt.name,
            "label": receipt.label,
            "domain": receipt.domain,
            "action": receipt.action,
            "pid": receipt.pid,
        }));
    }
    table::print(
        &["HOST", "SERVICE", "LABEL", "DOMAIN", "ACTION", "PID"],
        &[vec![
            receipt.host,
            receipt.name,
            receipt.label,
            receipt.domain,
            receipt.action,
            receipt
                .pid
                .map_or_else(|| "-".to_string(), |pid| pid.to_string()),
        ]],
    );
    if let Some(audited) = receipt.audited.as_deref() {
        println!("audit record {audited}");
    }
    Ok(())
}

/// The pass itself, for every caller that asserts a unit and reads the
/// receipt rather than printing it: `service ensure`, and placement standby
/// preparing a host for a profile.
pub(crate) async fn ensure_unit(options: EnsureOptions<'_>) -> Result<EnsureReceipt, CmdError> {
    let reason = options.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host has to run this unit; it is recorded beside the \
             registry document this command declares the unit in",
        ));
    }
    let target = crate::cli::canonical_host(options.host).await?;
    let host = target.name.clone();
    if options.as_launch_agent && !target.release_platform.starts_with("darwin") {
        return Err(CmdError::usage("--as-launch-agent is Darwin-only"));
    }

    // Resolve the operator-facing name against both declarations that may
    // supply a stable init-system identity. The service catalog owns authored
    // services; the managed-product catalog owns units that execute a delivered
    // product binary.
    let declared = service::declared_services(&target);
    let catalog_entry = crate::deploy::service_catalog::lookup(options.name).map_err(|error| {
        CmdError::click(error).stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let catalog_unit = catalog_entry.as_ref().and_then(|entry| entry.unit.clone());
    let managed_unit = canonical_managed_unit(options.name, &target.name)?;
    let canonical_unit = match (catalog_unit, managed_unit) {
        (Some(service_unit), Some(product_unit)) if service_unit != product_unit => {
            return Err(CmdError::click(format!(
                "service and managed-product declarations disagree about {}: {} versus {}",
                options.name, service_unit, product_unit
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        (Some(unit), _) | (_, Some(unit)) => Some(unit),
        (None, None) => None,
    };
    // A unit that runs a catalog product's program under a label that is not
    // that product's unit runs beside the process that replaced it the moment
    // it is loaded again, so no declaration may bring one back.
    if catalog_entry.is_none() {
        let named = declared
            .iter()
            .find(|candidate| candidate.matches(options.name));
        if let Some(existing) = named {
            if let Some(owner) =
                service::declared_owner(&target, existing).map_err(CmdError::click)?
            {
                return Err(CmdError::refused(
                    crate::deploy::service_catalog::retired_sentence(existing.unit_id(), &owner),
                ));
            }
        }
    }
    // The declarations this product's one process replaced on this host are
    // withdrawn in the same write: every one that runs its program under
    // another label. The host Stado process's role units keep theirs until
    // their role is proven, and are never repaired meanwhile.
    let host_product = crate::deploy::service_catalog::host_process().map_err(CmdError::click)?;
    let replaced: Vec<String> = match catalog_entry.as_ref() {
        Some(entry) if entry.name != host_product.name => declared
            .iter()
            .filter(|candidate| {
                service::declared_owner(&target, candidate)
                    .ok()
                    .flatten()
                    .is_some_and(|owner| owner.name == entry.name)
            })
            .map(|candidate| candidate.unit_id().to_string())
            .collect(),
        _ => Vec::new(),
    };
    let existing = declared.iter().find(|candidate| {
        candidate.matches(options.name)
            || canonical_unit
                .as_deref()
                .is_some_and(|unit| candidate.matches(unit))
    });
    // The port is read from, or assigned into, the service directory before
    // the unit is rendered, so the unit and every consumer's marker name the
    // same number and no port is written in the catalog.
    let listen_port = match catalog_entry.as_ref() {
        Some(entry) => {
            crate::cli::directory::listen_port_for(entry, &target, &production_runner()).await?
        }
        None => None,
    };
    let (mut unit, unit_env) = program::resolved_unit(
        &target,
        &options,
        existing,
        catalog_entry.as_ref(),
        listen_port,
    )?;
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
            &crate::deploy::local_install::label(options.name),
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
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
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
    // A product runs as one process per host. A unit that would start a
    // catalog product's executable under any label but that product's own is
    // a second process of it, and is refused before the host is touched. A
    // declaration that already ran that executable may still be repaired
    // while its product absorbs it; no declaration may start doing so.
    if let Some(product) =
        crate::deploy::service_catalog::second_process_of(&plan.label, &unit.program)
            .map_err(CmdError::declaration)?
    {
        let executable = crate::deploy::service_catalog::executable_name(&unit.program);
        let already_ran = existing.is_some_and(|declared| {
            crate::deploy::service_catalog::executable_name(&declared.program) == executable
        });
        if !already_ran {
            return Err(CmdError::refused(
                crate::deploy::service_catalog::second_process_sentence(
                    &plan.label,
                    &unit.program,
                    &product,
                ),
            ));
        }
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
    // The program has to be on the host before any unit it replaces is
    // touched: otherwise a missing install unloads those units, fails, and
    // loads them again — a restart of everything it replaces, for nothing.
    if !crate::deploy::host_channel::remote_test(
        &target,
        &format!("-f {}", crate::deploy::shlex_quote(&unit.program)),
        &runner,
    )
    .await
    .map_err(click)?
    {
        return Err(CmdError::refused(format!(
            "{}: {} runs {}, which is not on the host; deliver the product's qualified \
             service release to {} before ensuring it. Nothing was retired or started",
            target.name, options.name, unit.program, target.name
        )));
    }
    // A product whose release carries an acquisition-scope catalog acquires
    // its credentials at its first start; on a host whose vault does not know
    // those scopes yet it crash-loops on 401s. Register them before anything
    // is retired or started, so a refusal leaves the host as it was.
    if let Some(scopes) = catalog_entry
        .as_ref()
        .and_then(|entry| entry.acquisition_scopes.as_deref())
    {
        let installed = crate::deploy::service_catalog::resolve_word(
            scopes,
            &crate::deploy::service_catalog::home_for(&target),
            Some(target.release_platform.as_str()),
            &target.name,
        );
        let registered =
            crate::cli::host::register_installed_acquisition_scopes(&target.name, &installed)
                .await?;
        eprintln!(
            "{}: acquisition scopes from {installed}: {}",
            target.name,
            registered.trim()
        );
    }
    let retired =
        predecessors::retire_before_ensure(&target, catalog_entry.as_ref(), &runner).await?;
    let outcome = match service::ensure_service(&target, &plan, &runner).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut error = click(error);
            let given_back = predecessors::reinstate_after_failed_ensure(
                &target,
                &plan.label,
                &retired,
                &runner,
            )
            .await;
            if !given_back.is_empty() {
                error.message = Some(format!(
                    "{}{given_back}",
                    error.message.as_deref().unwrap_or("the ensure failed")
                ));
            }
            return Err(error);
        }
    };
    if !outcome.succeeded() {
        let mut detail = format!(
            "{host}: could not ensure {}: {}",
            options.name,
            outcome.report.failure()
        );
        if !outcome.report.postcondition_held() {
            // An unmanaged copy of the same program may still own the
            // service port and prevent this declared unit from staying up.
            detail.push_str(
                ". `stado service list --unowned` names a process that may still hold its port",
            );
        }
        detail.push_str(
            &predecessors::reinstate_after_failed_ensure(&target, &plan.label, &retired, &runner)
                .await,
        );
        return Err(
            CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown)
        );
    }
    // The write below goes through this process's registry route; when the
    // unit just acted on carries that route, it is made after the restarted
    // resolver publishes `serving`.
    route::await_route(&target, &plan, &outcome, options.name)
        .map_err(|cause| CmdError::click(format!("{host}: {cause}")))?;

    let mut record = service::record_from_ensure(&host, options.name, &outcome, &now());
    record.program = unit.program;
    record.args = unit.args;
    record.env = unit_env.into_iter().collect();
    record.systemd_unit = unit.systemd_unit;
    let persisted =
        persist_ensure_record(&record, &already, &replaced, &outcome, &plan, reason, &host).await;
    let audited = persisted.map_err(|mut error| {
        let cause = error
            .message
            .as_deref()
            .unwrap_or("registry operation failed");
        error.failure = Some(
            error
                .failure
                .unwrap_or(crate::primitives::failure::FailureCode::Unknown),
        );
        let said = if outcome.report.detail.is_empty() {
            String::new()
        } else {
            format!(" (the host said: {})", outcome.report.detail)
        };
        error.message = Some(format!(
            "{host}: {} is running (action {}, pid {}){said}, but recording the completed \
             ensure failed: {cause}. No host action was repeated.",
            options.name,
            outcome.action,
            outcome.pid.trim(),
        ));
        error.json = options.as_json;
        error
    })?;

    predecessors::retire_after_ensure(&target, catalog_entry.as_ref(), &record, &runner).await?;
    Ok(EnsureReceipt {
        host,
        name: record.name.clone(),
        label: record.unit_id().to_string(),
        domain: outcome.domain_word().to_string(),
        action: outcome.action.clone(),
        pid: outcome.pid.trim().parse::<u32>().ok(),
        audited,
    })
}
