//! `stado builds usage`: how many builds this fleet really started inside a
//! window, who asked for each one, and what the day's own counter says.
//!
//! `stado builds budget` reports the count the registry holds, and that
//! count is written by the two paths that maintain it. On 2026-09-21 it read
//! `2 of 3 build job(s) used` while this fleet had started 58 builds in
//! twenty-four hours — 47 of them release-pipeline builds nothing recorded,
//! 7 from the poller, 4 typed by hand. A ceiling is only as true as the
//! count behind it, so the count needs a second, independent reading that
//! does not come from the same document.
//!
//! This is that reading. Every build is a queue job, and every build job
//! carries a run id naming who asked for it: [`POLLER_RUN_PREFIX`] for the
//! coordinator's poller, [`MANUAL_RUN_PREFIX`] for `stado builds run`, and
//! the release pipeline's own per-platform scope. The counts come from the
//! queue's run manifests, which outlive the job records the reaper removes;
//! a manifest that could not be read is named rather than counted as zero.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

use crate::cli::builds::print_json;
use crate::cli::release_submit::RELEASE_BUILD_RUN_SCOPE;
use crate::cli::CmdError;
use crate::queue::JobStorage;
use crate::scheduler::builds::{BuildBudget, MANUAL_RUN_PREFIX, POLLER_RUN_PREFIX};

/// The default window: one day, the span the question is asked over.
pub(in crate::cli::builds) const DEFAULT_HOURS: i64 = 24;

/// How this build was asked for, from the run id its submitter stamped.
///
/// `stable_run_id` writes `run-<scope>-<digest>`, so the scope is the middle
/// of the name and never its head: matching the bare scope counted nothing
/// on a fleet that had been compiling all night.
fn origin(run_id: &str) -> Option<&'static str> {
    let scoped = |scope: &str| run_id.starts_with(&format!("run-{scope}"));
    if scoped(POLLER_RUN_PREFIX) {
        Some(POLLER_RUN_PREFIX)
    } else if scoped(MANUAL_RUN_PREFIX) {
        Some(MANUAL_RUN_PREFIX)
    } else if scoped(RELEASE_BUILD_RUN_SCOPE) {
        Some(RELEASE_BUILD_RUN_SCOPE)
    } else {
        None
    }
}

pub(in crate::cli::builds) async fn usage(hours: i64, json: bool) -> Result<(), CmdError> {
    if hours <= i64::default() {
        return Err(CmdError::usage(
            "--hours must be a positive number of hours",
        ));
    }
    let now = Utc::now();
    let since = now - Duration::hours(hours);
    let store = JobStorage::new().await?;

    let mut counted: BTreeMap<String, i64> = BTreeMap::new();
    let mut unread: Vec<String> = Vec::new();
    let mut total = i64::default();
    let run_ids = crate::queue::runs::list_runs(&store)
        .await
        .map_err(|error| CmdError::click(format!("the queue's run manifests: {error}")))?;
    for run_id in run_ids {
        let Some(origin) = origin(&run_id) else {
            continue;
        };
        let manifest = match crate::queue::runs::read_run(&store, &run_id).await {
            Ok(Some(manifest)) => manifest,
            Ok(None) => continue,
            Err(error) => {
                unread.push(format!("{run_id}: {error}"));
                continue;
            }
        };
        let Some(created) = manifest
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        else {
            unread.push(format!(
                "{run_id}: the manifest carries no readable created_at"
            ));
            continue;
        };
        if created.with_timezone(&Utc) < since {
            continue;
        }
        let jobs = manifest
            .get("entries")
            .and_then(Value::as_array)
            .map_or(i64::from(true), |entries| entries.len() as i64);
        total += jobs;
        *counted.entry(origin.to_string()).or_default() += jobs;
    }

    // The registry's own count beside this one. They disagree exactly when a
    // submitter spends the budget without recording it, which is the failure
    // this report exists to surface.
    let (document, _generation) = crate::cli::registry::fetch_versioned_document().await?;
    let budget = BuildBudget::read(&document, now);

    if json {
        return print_json(&json!({
            "schema": "stado.builds-usage.v1",
            "window_hours": hours,
            "since": since.to_rfc3339(),
            "observed": {"total": total, "by_origin": counted},
            "budget": {
                "day": budget.day,
                "used": budget.used,
                "limit": budget.limit,
                "remaining": budget.remaining(),
            },
            "unread": unread,
        }));
    }

    println!("builds in the last {hours}h: {total}");
    for (origin, asked) in &counted {
        println!("  {origin}\t{asked}");
    }
    if counted.is_empty() {
        println!("  (no build job inside the window; the queue's run manifests were read)");
    }
    println!(
        "today's counter: {} of {} used on {} (UTC), {} left",
        budget.used,
        budget.limit,
        budget.day,
        budget.remaining()
    );
    if total > budget.used as i64 {
        println!(
            "the window holds {total} build(s) and the day's counter holds {}: a submitter \
             spent the budget without recording it",
            budget.used
        );
    }
    for refusal in &unread {
        println!("unread: {refusal}");
    }
    Ok(())
}
