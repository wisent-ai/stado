//! `stado overview` — one operator snapshot for queue, fleet, quota and money.
//!
//! One component per section of the snapshot. `queue` counts the job states
//! and reads the published billing blob, `fleet` turns the registry and the
//! live capacity rows into the host inventory, and `budgets` reads the policy
//! limits the fleet is held to plus the GCP Billing Budgets. `render` holds
//! the human-readable printer, one component per printed section. The JSON
//! document assembled here is the single shape both outputs read, so a
//! section that appears in `--json` and a section that prints cannot drift.

mod budgets;
mod fleet;
mod queue;
mod render;

use chrono::{SecondsFormat, Utc};
use serde_json::json;

use super::CmdError;
use crate::deploy::fleet_claim;
use crate::queue::JobStorage;
use crate::targets;

use budgets::read_budgets;
use fleet::fleet_snapshot;
use queue::{queue_counts, read_billing};
use render::print_human;

pub async fn run(as_json: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let registry = targets::load_registry_auto()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;

    // The claimability verdict reads the capacity prefix, so it is the one
    // reader of it here: `capacity::read_consumer_capacity` would have
    // deleted every row past its GC horizon on the way, and a report that
    // collects the evidence it is reporting turns "this host went quiet
    // seventeen hours ago" into "this host never said anything".
    let now = Utc::now();
    let (jobs, claim, billing_snapshot, budgets, quotas) = tokio::join!(
        queue_counts(&store),
        fleet_claim::read_fleet_claim(&store, &registry, now),
        read_billing(&store),
        read_budgets(&store),
        crate::scheduler::quota::summarize_quotas(&store),
    );

    let jobs = jobs?;
    let claim = claim.map_err(|err| CmdError::click(err.to_string()))?;
    let billing_snapshot = billing_snapshot?;
    let budgets = budgets;
    let quotas = match quotas {
        Ok(summary) => serde_json::to_value(summary)?,
        Err(err) => json!({"status": "error", "detail": err.to_string()}),
    };
    // What each host CAN do, beside the fact that it is alive. An overview that
    // reports only liveness reads as a healthy fleet while the work it exists
    // for cannot run anywhere -- which is exactly how a browser login stayed
    // impossible for weeks behind three green workers.
    let measurements = crate::cli::registry::load_capability_measurements(&store)
        .await
        .unwrap_or_default();
    let fleet = fleet_snapshot(&registry, &claim.live_consumers(), &measurements, &claim);
    let document = json!({
        "generated_at": now.to_rfc3339_opts(SecondsFormat::Micros, false),
        "jobs": jobs,
        "fleet": fleet,
        "claimability": claim.to_report(),
        "quota": quotas,
        "billing": billing_snapshot,
        "budgets": budgets,
    });

    if as_json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        print_human(&document, &claim);
    }
    Ok(())
}
