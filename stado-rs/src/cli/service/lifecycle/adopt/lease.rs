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
    let mut lease = None;
    for _ in 0..300 {
        lease = crate::autonomy::storage::acquire_placement_lease(
            &store,
            &subject,
            &decision,
            "service-lifecycle",
            1800,
            chrono::Utc::now(),
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if lease.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let lease = lease.ok_or_else(|| {
        CmdError::click(format!(
            "{subject} stayed under another mutation lease for 300 seconds"
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

#[derive(Clone)]
pub(super) struct ReconcilerFence {
    pub(super) baseline_report: Option<String>,
    pub(super) timeout_seconds: u64,
}

fn active_coordinator_interval(document: &Value) -> Option<u64> {
    document
        .get("coordinators")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|coordinator| coordinator.get("active").and_then(Value::as_bool) == Some(true))
        .filter_map(|coordinator| coordinator.get("interval_seconds").and_then(Value::as_u64))
        .max()
        .map(|seconds| seconds.clamp(15, 600))
}

async fn reconciler_report_id(store: &JobStorage) -> Result<Option<String>, CmdError> {
    crate::autonomy::storage::read_json::<
        crate::autonomy::service_reconciler::ServiceReconcileReport,
    >(
        store,
        crate::autonomy::service_reconciler::LATEST_REPORT,
    )
    .await
    .map(|report| report.map(|report| report.created_at))
    .map_err(|error| CmdError::click(error.to_string()))
}

pub(super) async fn capture_reconciler_fence(
    document: &Value,
) -> Result<Option<ReconcilerFence>, CmdError> {
    let Some(interval) = active_coordinator_interval(document) else {
        return Ok(None);
    };
    let store = beacon_store().await?;
    Ok(Some(ReconcilerFence {
        baseline_report: reconciler_report_id(&store).await?,
        timeout_seconds: interval.saturating_mul(2).saturating_add(60).min(900),
    }))
}

pub(super) async fn wait_for_reconciler_fence(
    fence: Option<&ReconcilerFence>,
) -> Result<(), CmdError> {
    let Some(fence) = fence else {
        return Ok(());
    };
    let store = beacon_store().await?;
    let started = tokio::time::Instant::now();
    let timeout = std::time::Duration::from_secs(fence.timeout_seconds);
    loop {
        let read_error = match reconciler_report_id(&store).await {
            Ok(Some(current)) if Some(&current) != fence.baseline_report.as_ref() => return Ok(()),
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        };
        if started.elapsed() >= timeout {
            let detail = read_error
                .map(|error| format!("; the last report read failed: {error}"))
                .unwrap_or_default();
            return Err(CmdError::click(format!(
                "the active coordinator published no newer service-reconcile report within {} seconds{detail}",
                fence.timeout_seconds
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Remove a declaration before stopping its unit.
///
/// A coordinator from before the per-service lease can already hold an old
/// snapshot. The report fence waits until that pass has published its result;
/// after that publication no action from the old snapshot remains in flight,
/// while every later pass sees the withdrawn declaration.
pub(super) async fn suspend_service_declaration(
    host: &str,
    unit: &str,
) -> Result<(ManagedService, String, Option<ReconcilerFence>), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let fence = capture_reconciler_fence(&document).await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    Ok((removed, generation, fence))
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
