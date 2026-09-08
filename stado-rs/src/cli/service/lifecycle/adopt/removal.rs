//! `service retire` and `service remove`: the two ways a declaration is
//! withdrawn, and the directory half both of them have to drop with it.

use super::*;

/// Remove the directory half of one declaration. `service declare` writes the
/// target placeholder and `service_directory.services.<name>` in one registry
/// update; retire/remove must drop both in the same update or the validator
/// correctly refuses a directory entry pointing at no managed service.
///
/// Dropping an entry is a directory change, so it advances the publication
/// counter. It did not, and a consumer holding the entry that was just
/// removed saw a generation telling it its copy was current.
fn remove_directory_declaration(document: &mut Value, name: &str) {
    let Some(services) = document
        .get_mut("service_directory")
        .and_then(Value::as_object_mut)
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if services.remove(name).is_none() {
        return;
    }
    // A directory that cannot carry a counter is a document this command did
    // not write and must not silently repair; the removal still stands.
    let _ = crate::service_resolution::advance_generation(document);
}

async fn retirement_failure(service: &ManagedService, failure: String) -> CmdError {
    match restore_service_declaration(service).await {
        Ok(generation) => CmdError::click(format!(
            "{failure}; the registry declaration was restored at generation {generation}"
        )),
        Err(restore) => CmdError::click(format!(
            "{failure}; restoring the registry declaration also failed: {restore}"
        )),
    }
}

async fn finish_directory_retirement(
    service: &ManagedService,
    query: &str,
) -> Result<String, CmdError> {
    registry::commit_document(|document| {
        let mut next = document.clone();
        remove_directory_declaration(&mut next, &service.name);
        if service.unit_id() != service.name {
            remove_directory_declaration(&mut next, service.unit_id());
        }
        if query != service.name && query != service.unit_id() {
            remove_directory_declaration(&mut next, query);
        }
        Ok(next)
    })
    .await
}

/// Repeat only a host-confirmed retirement whose immediate end-state probe
/// caught one last external start. The declaration stays withdrawn for the
/// whole loop, and every pass reapplies the init-system fence; transport
/// failures and explicit host refusals are never retried.
async fn retire_service_stably(
    target: &crate::targets::ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &crate::deploy::Runner,
) -> Result<service::RemoteReport, DeployError> {
    let mut report = service::retire_service(target, service, sudo_password, runner).await?;
    for _ in 1..6 {
        if report.succeeded("retired")
            || report.status != "retired"
            || report.postcondition_state != host_channel::POSTCONDITION_UNMET
        {
            return Ok(report);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        report = service::retire_service(target, service, sudo_password, runner).await?;
    }
    Ok(report)
}

pub(crate) async fn retire(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be retired. Adopt it first if you need it under registry \
             management."
        )));
    }
    let runner = production_runner();
    let sudo_password = if UnitDomain::from_path(&found.path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    with_service_mutation_lease(&found, || async {
        let (removed, _, fence) = suspend_service_declaration(host, unit).await?;

        if removed.unit_id().is_empty() && removed.path.is_empty() {
            let generation = finish_directory_retirement(&removed, unit).await?;
            return render_mutation("retired", &removed, &generation, None, json);
        }
        if let Err(error) = wait_for_reconciler_fence(fence.as_ref()).await {
            let failure =
                format!("{host}: could not fence {unit} from the active coordinator: {error}");
            return Err(retirement_failure(&removed, failure).await);
        }

        let report =
            match retire_service_stably(&target, &removed, sudo_password.as_deref(), &runner).await
            {
                Ok(report) => report,
                Err(error) => {
                    let failure = format!("{host}: could not stop {unit}: {error}");
                    return Err(retirement_failure(&removed, failure).await);
                }
            };
        if !report.succeeded("retired") {
            let failure = format!("{host}: could not stop {unit}: {}", report.failure());
            return Err(retirement_failure(&removed, failure).await);
        }

        let generation = finish_directory_retirement(&removed, unit).await?;
        render_mutation(
            "retired",
            &removed,
            &generation,
            Some(&report.to_json()),
            json,
        )
    })
    .await
}

/// `service remove`: withdraw the declaration while holding the same mutation
/// lease as the autonomy reconciler, stop the unit, then remove its directory
/// entry and declared unit file. The file path comes from the registry rather
/// than operator input.
///
/// Partial states are said, not hidden: a stopped-and-forgotten service whose
/// file the channel may not delete is `retired` with the file named, and the
/// command exits non-zero because the asked-for end state did not happen.
pub(crate) async fn remove(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be removed. Adopt it first if you need it under registry \
             management."
        )));
    }
    let path = found.path.clone();
    let runner = production_runner();
    let sudo_password = if UnitDomain::from_path(&path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    with_service_mutation_lease(&found, || async {
        let (removed, _, fence) = suspend_service_declaration(host, unit).await?;

        if removed.unit_id().is_empty() && path.is_empty() {
            let generation = finish_directory_retirement(&removed, unit).await?;
            if json {
                return print_json(&json!({
                    "target": target.name,
                    "unit": unit,
                    "action": "removed",
                    "generation": generation,
                    "file": {"path": "", "status": "absent", "detail": Value::Null},
                    "report": Value::Null,
                }));
            }
            return render_mutation("removed", &removed, &generation, None, false);
        }
        if let Err(error) = wait_for_reconciler_fence(fence.as_ref()).await {
            let failure = format!(
                "{host}: could not fence {unit} from the active coordinator: {error}; its file was not touched"
            );
            return Err(retirement_failure(&removed, failure).await);
        }

        let report =
            match retire_service_stably(&target, &removed, sudo_password.as_deref(), &runner).await
            {
                Ok(report) => report,
                Err(error) => {
                    let failure =
                        format!("{host}: could not stop {unit}: {error}; its file was not touched");
                    return Err(retirement_failure(&removed, failure).await);
                }
            };
        if !report.succeeded("retired") {
            let failure = format!(
                "{host}: could not stop {unit}: {}; its file was not touched",
                report.failure()
            );
            return Err(retirement_failure(&removed, failure).await);
        }

        let generation = finish_directory_retirement(&removed, unit).await?;

        // The registry is already clean: the file half runs last, because a
        // failed delete must leave a service the fleet can still see, not a file
        // nobody declared. Its report is the second document of the answer.
        let file = crate::cli::host::remove_file_document(&target.name, &path).await;
        if json {
            let (file_status, file_detail) = match &file {
                Ok(outcome) => (outcome.status.clone(), outcome.detail.clone()),
                Err(error) => ("failed".to_string(), Some(error.to_string())),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": target.name,
                    "unit": removed.unit_id(),
                    "action": if file.is_ok() { "removed" } else { "retired" },
                    "generation": generation,
                    "report": report.to_json(),
                    "file": {
                        "path": path,
                        "status": file_status,
                        "detail": file_detail,
                    },
                }))?
            );
        } else {
            render_mutation(
                if file.is_ok() { "removed" } else { "retired" },
                &removed,
                &generation,
                Some(&report.to_json()),
                false,
            )?;
            match &file {
                Ok(outcome) => println!("file {}: {}", outcome.status, outcome.path),
                Err(error) => eprintln!("file failed: {error}"),
            }
        }
        file.map(|_| ())
    })
    .await
}
