//! `stado placement relief` — the plan the autonomy cycle's placement relief
//! would act on right now, from the same registry and the same host memory
//! publications, without the mutation gate. An operator reads here why a
//! profile stayed on a host over its watermark, or where the next tick will
//! move it, before the tick does.

use chrono::Utc;
use serde_json::json;

use crate::autonomy::placement_relief::{host_memory, plan, ReliefRow, LATEST_REPORT};
use crate::cli::{registry, CmdError};
use crate::queue::JobStorage;

pub(super) async fn relief(json_output: bool) -> Result<(), CmdError> {
    let now = Utc::now();
    let store = JobStorage::new().await?;
    let (document, _generation) = registry::fetch_versioned_document().await?;
    let parsed = crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let publications = crate::queue::capacity::read_publications(&store).await?;
    let hosts = host_memory(&parsed, &publications, now);
    let previous = crate::autonomy::storage::read_json::<
        crate::autonomy::placement_relief::ReliefReport,
    >(&store, LATEST_REPORT)
    .await?;
    let relocations = previous
        .as_ref()
        .map(|report| report.relocations.clone())
        .unwrap_or_default();
    // The same window the tick keeps: an operator reading this has to see the
    // profile the next tick will move, and a host that dipped below its
    // watermark two minutes ago is still pressured to both of them.
    let pressure_seen = previous
        .as_ref()
        .map(|report| report.pressure_seen.clone())
        .unwrap_or_default();
    let rows: Vec<ReliefRow> = plan(
        &document,
        &parsed,
        &hosts,
        &relocations,
        &pressure_seen,
        now,
    )
    .map_err(CmdError::click)?
    .into_iter()
    .map(|outcome| outcome.row)
    .collect();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "hosts": hosts,
                "rows": rows,
                "last_report": previous,
            }))?
        );
        return Ok(());
    }
    for (host, memory) in &hosts {
        println!("{host}\t{}", memory.describe());
    }
    for row in &rows {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            row.profile,
            row.placed_on.as_deref().unwrap_or("-"),
            row.classification,
            row.destination.as_deref().unwrap_or("-"),
            row.detail
        );
        for candidate in &row.candidates {
            println!(
                "\t{}\t{}\t{}\t{}",
                candidate.host,
                if candidate.declared {
                    "declared"
                } else {
                    "registered"
                },
                candidate.verdict,
                candidate
                    .memory
                    .as_ref()
                    .map_or_else(|| "no publication".to_string(), |memory| memory.describe())
            );
        }
    }
    if let Some(report) = previous {
        println!(
            "last tick {} ({:?}): planned {} relocated {} blocked {} failures {}",
            report.created_at,
            report.mode,
            report.summary.planned,
            report.summary.relocated,
            report.summary.blocked,
            report.summary.failures
        );
    }
    Ok(())
}
