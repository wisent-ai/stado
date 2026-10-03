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

/// Withdraw the managed record and its logical directory routes in one
/// versioned registry write after the host has stopped the unit. The caller
/// holds the per-unit lease against the autonomy reconciler throughout.
pub(super) async fn withdraw_service_declaration(
    host: &str,
    unit: &str,
) -> Result<(ManagedService, String), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    retire_directory_routes(&mut document, host, &removed)?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    Ok((removed, generation))
}

/// A replacement catalog unit takes over the route only if it is declared on
/// the same host. Otherwise, withdraw every route whose active host matches
/// and whose `managed_service` names the retired service by its declared
/// name or by its unit id, regardless of the route's own name.
fn retire_directory_routes(
    document: &mut Value,
    host: &str,
    removed: &ManagedService,
) -> Result<(), CmdError> {
    let retired = removed.unit_id();
    let names_retired = |reference: &str| {
        reference == removed.name || (!retired.is_empty() && reference == retired)
    };
    let replacement = crate::deploy::service_catalog::all()
        .map_err(CmdError::click)?
        .into_iter()
        .find(|entry| {
            entry.retired_units.iter().any(|unit| names_retired(unit))
                || entry
                    .role_units
                    .iter()
                    .any(|role| names_retired(&role.unit))
        })
        .and_then(|entry| entry.unit)
        .filter(|unit| {
            document
                .get("targets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
                .and_then(|target| target.get("services"))
                .and_then(Value::as_array)
                .is_some_and(|services| {
                    services.iter().filter_map(Value::as_object).any(|record| {
                        service::ManagedService::from_record(host, record).matches(unit)
                    })
                })
        });
    let Some(entries) = document
        .get_mut("service_directory")
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let mut changed = false;
    entries.retain(|_, entry| {
        let matches = entry.get("active_host").and_then(Value::as_str) == Some(host)
            && entry
                .get("managed_service")
                .and_then(Value::as_str)
                .is_some_and(names_retired);
        if !matches {
            return true;
        }
        changed = true;
        if let Some(unit) = &replacement {
            entry["managed_service"] = Value::from(unit.as_str());
            true
        } else {
            false
        }
    });
    if changed {
        crate::service_resolution::advance_generation(document).map_err(click)?;
    }
    Ok(())
}
