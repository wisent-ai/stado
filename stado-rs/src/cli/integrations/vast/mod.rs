//! The Vast.ai adapter behind `stado market --provider vast`.
//!
//! `list`, `unlist`, `status`, `monitor`, the `auto-list` loop and the
//! `readiness` verdict, each against Vast.ai's host API. Monitor and
//! auto-list read the queue through [`JobStorage`], which honors
//! `WC_STORAGE_BACKEND`. A dry run builds no client: `--dry-run` promises the
//! toggle decisions without calling the marketplace, so it needs no
//! credential.

mod readiness;

use serde_json::json;

use crate::cli::{CmdError, MarketCommands, MarketProvider};
use crate::providers::vast::{self, AutoListParams, ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

/// Dispatch one `market` subcommand to the marketplace it names.
pub(crate) async fn dispatch(command: &MarketCommands) -> Result<(), CmdError> {
    match command {
        MarketCommands::List {
            provider: MarketProvider::Vast,
            price_gpu,
            price_disk,
            price_min_bid,
            json,
        } => list(*price_gpu, *price_disk, *price_min_bid, *json).await,
        MarketCommands::Unlist {
            provider: MarketProvider::Vast,
            json,
        } => unlist(*json).await,
        MarketCommands::Status {
            provider: MarketProvider::Vast,
            json,
        } => status(*json).await,
        MarketCommands::Readiness {
            provider: MarketProvider::Vast,
            vault_host,
            no_vault_check,
            json,
        } => readiness::report(vault_host.clone(), *no_vault_check, *json).await,
        MarketCommands::Monitor {
            provider: MarketProvider::Vast,
            bucket,
            json,
        } => {
            monitor(
                bucket.as_deref().unwrap_or_else(|| crate::config::bucket()),
                *json,
            )
            .await
        }
        MarketCommands::AutoList {
            provider: MarketProvider::Vast,
            idle_window_s,
            poll_interval_s,
            price_gpu,
            max_duration_s,
            dry_run,
            once,
        } => {
            auto_list(
                *idle_window_s,
                *poll_interval_s,
                *price_gpu,
                *max_duration_s,
                *dry_run,
                *once,
            )
            .await
        }
    }
}

/// A marketplace failure as the command reports it, with its class: a
/// queue-state failure is classed by the store, every other by
/// [`VastError::failure_code`], so no market refusal reads as unattributed.
fn cmd_err(exc: VastError) -> CmdError {
    match exc {
        VastError::Storage(error) => CmdError::from(error),
        other => match other.failure_code() {
            Some(code) => CmdError::click(other.to_string()).stating(code),
            None => CmdError::click(other.to_string()),
        },
    }
}

async fn list(
    price_gpu: f64,
    price_disk: f64,
    price_min_bid: Option<f64>,
    json_output: bool,
) -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client
        .list_machine(&ListMachineParams {
            price_gpu,
            price_disk,
            price_min_bid,
            ..ListMachineParams::default()
        })
        .await
        .map_err(cmd_err)?;
    crate::cli::print_answer(&result, json_output)?;
    Ok(())
}

async fn unlist(json_output: bool) -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client.unlist_machine().await.map_err(cmd_err)?;
    crate::cli::print_answer(&result, json_output)?;
    Ok(())
}

async fn status(json_output: bool) -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client.machine_status().await.map_err(cmd_err)?;
    crate::cli::print_answer(&result, json_output)?;
    Ok(())
}

/// Python `datetime.utcnow().isoformat() + "Z"` (microseconds always).
fn now_utc_iso_z() -> String {
    format!(
        "{}Z",
        chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
    )
}

/// One snapshot: what Vast says about our machine, what the queue holds, and
/// which Skarbiec channel the credential came through.
///
/// The credential block is here because the snapshot's `vast_machine.error`
/// was the only signal an operator had, and it named neither the consumer
/// that asked nor the channel it asked on.
async fn monitor(bucket: &str, json_output: bool) -> Result<(), CmdError> {
    let reading = vast::read_vast_api_key().await;
    let vast_machine = match reading.key.clone() {
        None => json!({"error": reading.refusal()}),
        Some(key) => match VastClient::new(key).machine_status().await {
            Ok(machine) => machine,
            Err(VastError::Config(message)) => json!({"error": message}),
            Err(exc) => return Err(cmd_err(exc)),
        },
    };
    let store = JobStorage::with_bucket(bucket).await?;
    let hostname = vast::system_hostname();
    let capacity = vast::read_capacity_snapshot(&store, &hostname).await;
    // Python list_blobs(prefix=..., max_results=512) counts.
    let queued = store.list_paths("queue/", 512).await?.len();
    let running = store.list_paths("running/", 512).await?.len();
    crate::cli::print_answer(
        &json!({
            "now": now_utc_iso_z(),
            "hostname": hostname,
            "credential": serde_json::to_value(&reading)?,
            "vast_machine": vast_machine,
            "wisent_capacity": capacity,
            "wisent_queue": queued,
            "wisent_running": running,
        }),
        json_output,
    )?;
    Ok(())
}

async fn auto_list(
    idle_window_s: i64,
    poll_interval_s: Option<u64>,
    price_gpu: f64,
    max_duration_s: i64,
    dry_run: bool,
    once: bool,
) -> Result<(), CmdError> {
    let reading = vast::read_vast_api_key().await;
    let client = reading.key.clone().map(VastClient::new);
    if client.is_none() {
        if !dry_run {
            return Err(CmdError::click(reading.refusal())
                .stating(crate::primitives::failure::FailureCode::Config));
        }
        eprintln!("[vast] {}", reading.refusal());
    }
    // Python auto_list_loop's default bucket is the literal
    // "wisent-compute" (the CLI exposes no --bucket here).
    let store = JobStorage::with_bucket("wisent-compute").await?;
    let hostname = vast::system_hostname();
    let params = AutoListParams {
        idle_window_s,
        poll: poll_interval_s.map(std::time::Duration::from_secs),
        price_gpu,
        dry_run,
        once,
        duration_s: if max_duration_s > 0 {
            Some(max_duration_s)
        } else {
            None
        },
    };
    vast::auto_list_loop(client.as_ref(), &store, &hostname, params, |m| {
        println!("{m}")
    })
    .await
    .map_err(cmd_err)
}
