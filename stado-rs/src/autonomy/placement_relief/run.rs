//! One pass: the plan, then each due move through the one shared mutation
//! gate, then the report the pass leaves behind.

use chrono::{SecondsFormat, Utc};

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::queue::{JobStorage, StorageError};

use super::{
    host_memory, plan, words, ReliefReport, ReliefSummary, LATEST_REPORT, MAX_RELOCATIONS_PER_TICK,
    REPORT_PREFIX, SCHEMA_VERSION,
};

pub async fn reconcile(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    log: &dyn Fn(&str),
) -> Result<ReliefReport, StorageError> {
    let now = Utc::now();
    let created_at = now.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let decision_id = format!("placement-relief-{}", created_at.replace(':', "-"));
    let previous =
        crate::autonomy::storage::read_json::<ReliefReport>(store, LATEST_REPORT).await?;
    let mut relocations = previous
        .map(|report| report.relocations)
        .unwrap_or_default();

    let (document, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;
    let registry = crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;
    let publications = crate::queue::capacity::read_publications(store).await?;
    let hosts = host_memory(&registry, &publications, now);
    let outcomes = plan(&document, &registry, &hosts, &relocations, now)
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;

    let mut summary = ReliefSummary {
        profiles: outcomes.len(),
        ..ReliefSummary::default()
    };
    let mut rows = Vec::with_capacity(outcomes.len());
    let mut relocated = usize::default();
    let authority = crate::cli::placement::local_is_authority(&document, &registry);
    for outcome in outcomes {
        let mut row = outcome.row;
        if row.candidates.len() > usize::default() {
            summary.pressured += 1;
        }
        let Some(due) = outcome.due else {
            if row.classification != words::SETTLED {
                summary.blocked += 1;
            }
            rows.push(row);
            continue;
        };
        summary.planned += 1;
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            row.classification = words::PLANNED.to_string();
            row.detail = if policy.emergency_paused {
                format!(
                    "{}; mutation blocked by autonomy emergency pause",
                    row.detail
                )
            } else {
                format!(
                    "{}; report mode: the move was planned but not executed",
                    row.detail
                )
            };
            rows.push(row);
            continue;
        }
        if relocated >= MAX_RELOCATIONS_PER_TICK || relocated >= policy.limits.max_actions_per_tick
        {
            row.classification = words::ACTION_LIMIT.to_string();
            row.detail = format!("{}; this tick already spent its relocation", row.detail);
            summary.blocked += 1;
            rows.push(row);
            continue;
        }
        let control = crate::autonomy::storage::load_control(store).await?;
        if control.emergency_paused || control.circuit_open_at(Utc::now()) {
            row.classification = words::CONTROL_BLOCKED.to_string();
            row.detail = format!(
                "{}; autonomy pause or circuit breaker became active",
                row.detail
            );
            summary.blocked += 1;
            rows.push(row);
            continue;
        }
        match &authority {
            Ok(Some(false)) => {
                row.classification = words::AUTHORITY_ELSEWHERE.to_string();
                row.detail = format!(
                    "{}; only the directory authority commits a placement transaction",
                    row.detail
                );
                summary.blocked += 1;
                rows.push(row);
                continue;
            }
            Err(error) => {
                row.classification = words::AUTHORITY_ELSEWHERE.to_string();
                row.detail = format!("{}; {error}", row.detail);
                summary.blocked += 1;
                rows.push(row);
                continue;
            }
            Ok(_) => {}
        }
        let lease_subject = format!("placement:{}", due.profile.name);
        let Some(lease) = crate::autonomy::storage::acquire_placement_lease(
            store,
            &lease_subject,
            &decision_id,
            "placement-relief",
            policy.limits.decision_ttl_seconds,
            Utc::now(),
        )
        .await?
        else {
            row.classification = words::LEASE_BLOCKED.to_string();
            row.detail = format!("{}; another reconciler owns this profile", row.detail);
            summary.blocked += 1;
            rows.push(row);
            continue;
        };
        relocated += 1;
        let mut result = crate::cli::placement::relocate(
            document.clone(),
            generation.clone(),
            due.profile,
            &due.to_host,
        )
        .await;
        match crate::autonomy::storage::release_placement_lease(store, &lease_subject, &lease.token)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                result = Err(
                    "relocation finished, but mutation lease ownership changed before release"
                        .to_string(),
                );
            }
            Err(error) => {
                result = Err(format!(
                    "relocation finished, but mutation lease release failed: {error}"
                ));
            }
        }
        match result {
            Ok(receipt) => {
                row.classification = words::RELOCATED.to_string();
                row.transaction_id = receipt.transaction_id;
                row.detail = format!(
                    "{}; moved {} from {} to {} ({})",
                    row.detail,
                    receipt.profile,
                    receipt.from_host,
                    receipt.to_host,
                    receipt.registry_generation.map_or_else(
                        || receipt.status.to_string(),
                        |generation| { format!("registry generation {generation}") }
                    )
                );
                relocations.insert(row.profile.clone(), created_at.clone());
                summary.relocated += 1;
                crate::autonomy::storage::record_mutation_outcome(
                    store,
                    true,
                    None,
                    policy.limits.circuit_breaker_failures,
                    policy.limits.circuit_breaker_cooldown_seconds,
                )
                .await?;
            }
            Err(error) => {
                row.classification = words::RELOCATION_FAILED.to_string();
                row.detail = format!("{}; {error}", row.detail);
                summary.failures += 1;
                crate::autonomy::storage::record_mutation_outcome(
                    store,
                    false,
                    Some(&error),
                    policy.limits.circuit_breaker_failures,
                    policy.limits.circuit_breaker_cooldown_seconds,
                )
                .await?;
            }
        }
        rows.push(row);
    }

    let report = ReliefReport {
        schema_version: SCHEMA_VERSION,
        decision_id,
        created_at: created_at.clone(),
        mode: policy.mode,
        summary,
        rows,
        relocations,
    };
    crate::autonomy::storage::write_json(store, LATEST_REPORT, &report, false).await?;
    crate::autonomy::storage::write_json(
        store,
        &format!("{REPORT_PREFIX}/{}.json", created_at.replace(':', "-")),
        &report,
        true,
    )
    .await?;
    log(&format!(
        "autonomy placement relief: profiles={} pressured={} planned={} relocated={} blocked={} failures={}",
        report.summary.profiles,
        report.summary.pressured,
        report.summary.planned,
        report.summary.relocated,
        report.summary.blocked,
        report.summary.failures
    ));
    Ok(report)
}
