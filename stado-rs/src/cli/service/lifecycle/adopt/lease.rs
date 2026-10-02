//! The lease and report fence an operator lifecycle mutation holds against
//! the autonomy reconciler, and the declaration withdrawal it covers.

use super::*;

/// Serialize an operator lifecycle mutation with the autonomy reconciler.
///
/// A reconciler tick takes its service snapshot before it takes the per-unit
/// lease. Holding the same lease across withdrawal and the host action makes a
/// stale tick stop at that boundary instead of starting the unit between the
/// stop body and its postcondition probe.
pub(super) async fn with_service_mutation_subject<T, F, Fut>(
    host: &str,
    unit: &str,
    operation: F,
) -> Result<T, CmdError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, CmdError>>,
{
    let store = beacon_store().await?;
    let subject = format!("service:{host}:{unit}");
    let decision = format!(
        "service-lifecycle-{}",
        chrono::Utc::now().timestamp_micros()
    );
    let lease = crate::autonomy::storage::acquire_placement_lease(
        &store,
        &subject,
        &decision,
        "service-lifecycle",
        1800,
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    .ok_or_else(|| {
        CmdError::click(format!(
            "{subject} is held under another mutation lease; run the command again once that \
             mutation has finished"
        ))
    })?;
    let result = operation().await;
    let released =
        crate::autonomy::storage::release_placement_lease(&store, &subject, &lease.token).await;
    match (result, released) {
        (Ok(value), Ok(true)) => Ok(value),
        (Ok(_), Ok(false)) => Err(CmdError::click(format!(
            "{subject} changed lease ownership before the lifecycle operation completed"
        ))),
        (Ok(_), Err(error)) => Err(CmdError::click(format!(
            "{subject} completed, but releasing its mutation lease failed: {error}"
        ))),
        (Err(error), Ok(true)) => Err(error),
        (Err(error), Ok(false)) => Err(CmdError::click(format!(
            "{error}; {subject} changed lease ownership before failure cleanup completed"
        ))),
        (Err(error), Err(release)) => Err(CmdError::click(format!(
            "{error}; releasing the {subject} mutation lease also failed: {release}"
        ))),
    }
}

pub(super) async fn with_service_mutation_lease<T, F, Fut>(
    service: &ManagedService,
    operation: F,
) -> Result<T, CmdError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, CmdError>>,
{
    with_service_mutation_subject(&service.host, service.unit_id(), operation).await
}

/// Remove a declaration before stopping its unit. The caller holds the
/// per-unit mutation lease the autonomy reconciler also takes, so no
/// reconciler pass acts on the unit while it is stopped.
///
/// A unit a service-directory entry names as its `managed_service` on its
/// active host is handed to the catalog product that replaced it — its
/// `retired_units` or `role_units` name the unit — when that product's own
/// unit is declared on the same host, in the same write. Without that the
/// directory kept naming the predecessor and the retirement was refused
/// (`managed_service: is not declared on the active host`) for as long as
/// the entry existed, while the product's one process already served it.
pub(super) async fn suspend_service_declaration(
    host: &str,
    unit: &str,
) -> Result<(ManagedService, String), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    hand_directory_to_replacement(&mut document, host, removed.unit_id())?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    Ok((removed, generation))
}

fn hand_directory_to_replacement(
    document: &mut Value,
    host: &str,
    retired: &str,
) -> Result<(), CmdError> {
    let Some(replacement) = crate::deploy::service_catalog::all()
        .map_err(CmdError::click)?
        .into_iter()
        .find(|entry| {
            entry.retired_units.iter().any(|unit| unit == retired)
                || entry.role_units.iter().any(|role| role.unit == retired)
        })
    else {
        return Ok(());
    };
    let Some(unit) = replacement.unit.clone() else {
        return Ok(());
    };
    let declared_here = document
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        .and_then(|target| target.get("services"))
        .and_then(Value::as_array)
        .is_some_and(|services| {
            services
                .iter()
                .filter_map(Value::as_object)
                .any(|record| service::ManagedService::from_record(host, record).matches(&unit))
        });
    if !declared_here {
        return Ok(());
    }
    let Some(entries) = document
        .get_mut("service_directory")
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    for entry in entries.values_mut() {
        let on_host = entry.get("active_host").and_then(Value::as_str) == Some(host);
        let names_retired = entry.get("managed_service").and_then(Value::as_str) == Some(retired);
        if on_host && names_retired {
            entry["managed_service"] = Value::from(unit.clone());
        }
    }
    Ok(())
}

pub(super) async fn restore_service_declaration(
    service: &ManagedService,
) -> Result<String, CmdError> {
    let record = service.to_record();
    registry::commit_document(|document| {
        let already_restored = document
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|target| target.get("name").and_then(Value::as_str) == Some(&service.host))
            .and_then(|target| target.get("services"))
            .and_then(Value::as_array)
            .is_some_and(|services| services.iter().any(|candidate| candidate == &record));
        if already_restored {
            return Ok(document.clone());
        }
        let mut restored = document.clone();
        service::add_service(&mut restored, service).map_err(click)?;
        Ok(restored)
    })
    .await
}
