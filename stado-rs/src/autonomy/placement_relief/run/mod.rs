//! One pass: the plan, then each due move through the one shared mutation
//! gate, then the report the pass leaves behind.

use chrono::{SecondsFormat, Utc};

use crate::autonomy::policy::AutonomyPolicy;
use crate::queue::{JobStorage, StorageError};

use super::{host_memory, plan, words, ReliefReport, ReliefSummary, LATEST_REPORT, REPORT_PREFIX};

mod execute;
mod gates;

use execute::execute;
use gates::refusal;

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
    let relocations = previous
        .map(|report| report.relocations)
        .unwrap_or_default();

    let (document, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;
    let registry = crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;
    let publications = crate::queue::capacity::read_publications(store).await?;
    let hosts = host_memory(&registry, &publications, now);
    let outcomes = plan(&document, &registry, &hosts)
        .map_err(|error| StorageError::Other(format!("placement relief: {error}")))?;

    let mut report = ReliefReport {
        decision_id,
        created_at: created_at.clone(),
        mode: policy.mode,
        summary: ReliefSummary {
            profiles: outcomes.len(),
            ..ReliefSummary::default()
        },
        rows: Vec::with_capacity(outcomes.len()),
        relocations,
    };
    let mut relocated: Vec<String> = Vec::new();
    let authority = crate::cli::placement::local_is_authority(&document, &registry);
    for outcome in outcomes {
        let mut row = outcome.row;
        if !row.candidates.is_empty() {
            report.summary.pressured += 1;
        }
        let Some(due) = outcome.due else {
            if row.classification != words::SETTLED {
                report.summary.blocked += 1;
            }
            report.rows.push(row);
            continue;
        };
        report.summary.planned += 1;
        if let Some(refusal) = refusal(store, policy, &authority, &relocated, &due.action).await? {
            row.classification = refusal.classification;
            row.detail = format!("{}; {}", row.detail, refusal.detail);
            if refusal.blocked {
                report.summary.blocked += 1;
            }
            report.rows.push(row);
            continue;
        }
        let profile = due.profile.name.clone();
        if execute(
            store,
            policy,
            &document,
            &generation,
            due,
            &mut row,
            &mut report,
        )
        .await?
        {
            relocated.push(profile);
        }
        report.rows.push(row);
    }

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
