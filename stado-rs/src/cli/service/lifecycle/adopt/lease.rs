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
pub(super) async fn suspend_service_declaration(
    host: &str,
    unit: &str,
) -> Result<(ManagedService, String), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    Ok((removed, generation))
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
