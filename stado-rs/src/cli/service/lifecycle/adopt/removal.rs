//! `service retire` and `service remove`: the two ways a declaration is
//! withdrawn, and the directory half both of them have to drop with it.

use super::*;

pub(crate) async fn retire(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::refused(format!(
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
        if found.unit_id().is_empty() && found.path.is_empty() {
            let (removed, generation) = withdraw_service_declaration(host, unit).await?;
            return render_mutation("retired", &removed, &generation, None, json);
        }

        let report = service::retire_service(&target, &found, sudo_password.as_deref(), &runner)
            .await
            .map_err(|error| {
                CmdError::from(error).within(format!("{host}: could not stop {unit}"))
            })?;
        if !report.succeeded("retired") {
            return Err(CmdError::click(format!(
                "{host}: could not stop {unit}: {}",
                report.failure()
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let (removed, generation) =
            withdraw_service_declaration(host, unit)
                .await
                .map_err(|error| {
                    error
                        .within(format!(
                            "{host}: {unit} stopped, but declaration withdrawal failed"
                        ))
                        .also("the registry still declares it")
                })?;
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

/// `service remove`: stop the unit while holding the same mutation lease as
/// the autonomy reconciler, then withdraw its record and directory routes in
/// one registry write before deleting its declared unit file.
///
/// Partial states are said, not hidden: a stopped-and-forgotten service whose
/// file the channel may not delete is `retired` with the file named, and the
/// command exits non-zero because the asked-for end state did not happen.
pub(crate) async fn remove(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::refused(format!(
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
        if found.unit_id().is_empty() && path.is_empty() {
            let (removed, generation) = withdraw_service_declaration(host, unit).await?;
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

        let report = service::retire_service(&target, &found, sudo_password.as_deref(), &runner)
            .await
            .map_err(|error| {
                CmdError::from(error)
                    .within(format!("{host}: could not stop {unit}"))
                    .also("its file was not touched")
            })?;
        if !report.succeeded("retired") {
            return Err(CmdError::click(format!(
                "{host}: could not stop {unit}: {}; its file was not touched",
                report.failure()
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let (removed, generation) =
            withdraw_service_declaration(host, unit)
                .await
                .map_err(|error| {
                    error
                        .within(format!(
                            "{host}: {unit} stopped, but declaration withdrawal failed"
                        ))
                        .also("the registry still declares it and its file was not touched")
                })?;

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
