//! `stado cost report` / `stado cost estimate BATCH_FILE` — port of the
//! `cost` group in `stado/cli.py`, backed by
//! [`crate::scheduler::cost`].

use std::path::Path;

use crate::queue::submit::default_store;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost;

use super::{CmdError, CostCommands};

impl From<crate::scheduler::cost::Projection> for Vec<String> {
    /// Python cli.py `cost_estimate` echo lines.
    fn from(proj: crate::scheduler::cost::Projection) -> Self {
        let Some(projected) = proj.projected_cost_usd else {
            return vec![format!("cannot project: {}", proj.reason)];
        };
        vec![
            format!("jobs_in_batch:        {}", proj.jobs_in_batch),
            format!(
                "samples:              {} completed jobs in queue history",
                proj.samples
            ),
            format!("avg_cost_usd_per_job: ${:.4}", proj.avg_cost_usd_per_job),
            format!("projected_cost_usd:   ${projected:.2}"),
        ]
    }
}

/// Python `cost_report`: the formatted report lines.
pub(crate) async fn report_lines(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    Ok(cost::format_report(&cost::report(store).await?))
}

/// Python `cost_estimate`: the projection lines ("cannot project: ..."
/// when there is no history to base the projection on).
pub(crate) async fn estimate_lines(
    store: &JobStorage,
    batch_file: &Path,
) -> Result<Vec<String>, StorageError> {
    Ok(cost::project_batch(batch_file, store).await?.into())
}

pub(crate) async fn dispatch(sub: &CostCommands) -> Result<(), CmdError> {
    match sub {
        CostCommands::Prices { json } => {
            crate::cli::work::autonomy::show_report("prices", *json).await?;
        }
        CostCommands::Quote { job_ids, json } => {
            let store = default_store(crate::config::bucket()).await?;
            let report = crate::autonomy::cost::quote_jobs(&store, job_ids).await;
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Read at {}; every allocation quoted: {}",
                    report.created_at, report.complete
                );
                for row in &report.quotes {
                    match (&row.quote, &row.error) {
                        (Some(quote), None) => println!(
                            "{}: {} {} / {}; source={}, observed={}",
                            row.job_id,
                            quote.hourly_usd,
                            quote.currency,
                            quote.unit,
                            quote.source,
                            quote.observed_at
                        ),
                        (_, error) => println!("{}: unpriced; {:?}", row.job_id, error),
                    }
                }
                if let Some(error) = &report.book_error {
                    println!("Price book: {error}");
                }
                if let Some(error) = &report.inventory_error {
                    println!("Inventory: {error}");
                }
                println!(
                    "Price sources: {}",
                    serde_json::to_string_pretty(&report.price_sources)?
                );
                println!(
                    "Inventory sources: {}",
                    serde_json::to_string_pretty(&report.inventory_sources)?
                );
            }
        }
        CostCommands::Report { json } => {
            let store = default_store(crate::config::bucket()).await?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&cost::report(&store).await?)?
                );
            } else {
                for line in report_lines(&store).await? {
                    println!("{line}");
                }
            }
        }
        CostCommands::Estimate { batch_file, json } => {
            let store = default_store(crate::config::bucket()).await?;
            if *json {
                let projection = cost::project_batch(Path::new(batch_file), &store).await?;
                println!("{}", serde_json::to_string_pretty(&projection)?);
            } else {
                for line in estimate_lines(&store, Path::new(batch_file)).await? {
                    println!("{line}");
                }
            }
        }
        CostCommands::Allocation { json } => {
            crate::cli::work::autonomy::show_report("allocation", *json).await?;
        }
        CostCommands::Forecast { json } => {
            crate::cli::work::autonomy::show_report("forecast", *json).await?;
        }
        CostCommands::Anomalies { json } => {
            crate::cli::work::autonomy::show_report("anomalies", *json).await?;
        }
        CostCommands::Savings { json } => {
            crate::cli::work::autonomy::show_report("savings", *json).await?;
        }
    }
    Ok(())
}
