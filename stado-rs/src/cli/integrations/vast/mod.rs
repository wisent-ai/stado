//! `stado market`: the dispatch by `--provider`, and the Vast.ai adapter
//! behind `--provider vast`.
//!
//! `list`, `unlist`, `status`, `monitor`, the `follow` loop and the
//! `readiness` verdict, each against Vast.ai's host API. Monitor and
//! follow read the queue through [`JobStorage`], which honors
//! `WC_STORAGE_BACKEND`. A dry run builds no client: `--dry-run` promises the
//! toggle decisions without calling the marketplace, so it needs no
//! credential. `--provider stado` and `stado market peer` are answered by
//! [`super::peer`].

mod readiness;
mod readiness_text;

pub(crate) use readiness::read_vault;

use serde_json::json;

use crate::cli::{CmdError, MarketCommands, MarketProvider};
use crate::providers::vast::{self, AutoListParams, ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

/// Dispatch one `market` subcommand to the marketplace it names.
pub(crate) async fn dispatch(command: &MarketCommands) -> Result<(), CmdError> {
    if let MarketCommands::Peer(peer) = command {
        return super::peer::declare(peer).await;
    }
    if provider(command) == MarketProvider::Stado {
        return super::peer::dispatch(command).await;
    }
    if peer_named(command) {
        return Err(CmdError::usage(
            "--peer names a Stado fleet and is taken only with --provider stado",
        ));
    }
    match command {
        MarketCommands::List {
            price_gpu,
            price_disk,
            price_min_bid,
            json,
            ..
        } => {
            let Some(price_disk) = price_disk else {
                return Err(CmdError::usage(
                    "listing on Vast.ai needs --price-disk, the per-GB-month disk price in USD",
                ));
            };
            list(*price_gpu, *price_disk, *price_min_bid, *json).await
        }
        MarketCommands::Unlist { json, .. } => unlist(*json).await,
        MarketCommands::Status { json, .. } => status(*json).await,
        MarketCommands::Readiness {
            vault_host,
            no_vault_check,
            json,
            ..
        } => readiness::report(vault_host.clone(), *no_vault_check, *json).await,
        MarketCommands::Monitor { bucket, json, .. } => {
            // `--bucket` names the store when given; else it is the one the
            // configuration selects.
            let bucket = match bucket.as_deref() {
                Some(named) => named,
                None => crate::config::bucket(),
            };
            monitor(bucket, *json).await
        }
        MarketCommands::Follow {
            idle_window_s,
            poll_interval_s,
            price_gpu,
            price_disk,
            max_duration_s,
            dry_run,
            once,
            ..
        } => {
            let Some(price_disk) = price_disk else {
                return Err(CmdError::usage(
                    "following the queue on Vast.ai needs --price-disk, the per-GB-month disk price in USD",
                ));
            };
            auto_list(
                *idle_window_s,
                *poll_interval_s,
                (*price_gpu, *price_disk),
                *max_duration_s,
                *dry_run,
                *once,
            )
            .await
        }
        MarketCommands::Peer(_) => unreachable!("peer declarations were answered above"),
    }
}

/// The marketplace a provider-bearing subcommand names.
pub(crate) fn provider(command: &MarketCommands) -> MarketProvider {
    match command {
        MarketCommands::List { provider, .. }
        | MarketCommands::Unlist { provider, .. }
        | MarketCommands::Status { provider, .. }
        | MarketCommands::Readiness { provider, .. }
        | MarketCommands::Monitor { provider, .. }
        | MarketCommands::Follow { provider, .. } => *provider,
        MarketCommands::Peer(_) => MarketProvider::Stado,
    }
}

/// Whether the subcommand carries `--peer`.
fn peer_named(command: &MarketCommands) -> bool {
    match command {
        MarketCommands::List { peer, .. }
        | MarketCommands::Unlist { peer, .. }
        | MarketCommands::Status { peer, .. }
        | MarketCommands::Readiness { peer, .. }
        | MarketCommands::Monitor { peer, .. }
        | MarketCommands::Follow { peer, .. } => peer.is_some(),
        MarketCommands::Peer(_) => false,
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
            duration: None,
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
        None => json!({"error": vast::vast_refusal(&reading)}),
        Some(key) => match VastClient::new(key).machine_status().await {
            Ok(machine) => machine,
            Err(VastError::Config(message)) => json!({"error": message}),
            Err(exc) => return Err(cmd_err(exc)),
        },
    };
    let store = JobStorage::with_bucket(bucket).await?;
    let hostname = vast::system_hostname();
    let capacity = vast::read_capacity_snapshot(&store, &hostname).await;
    // Every queued and running job is counted; a count cut at a page size
    // nobody stated would read a full queue as a short one.
    let queued = store
        .list_paths("queue/", crate::queue::Listing::Every)
        .await?
        .len();
    let running = store
        .list_paths("running/", crate::queue::Listing::Every)
        .await?
        .len();
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

/// Run the Vast.ai bridge; the price pair is the per-GPU-hour and the
/// per-GB-month disk price in USD.
async fn auto_list(
    idle_window_s: i64,
    poll_interval_s: Option<u64>,
    (price_gpu, price_disk): (f64, f64),
    max_duration_s: i64,
    dry_run: bool,
    once: bool,
) -> Result<(), CmdError> {
    let reading = vast::read_vast_api_key().await;
    let client = reading.key.clone().map(VastClient::new);
    if client.is_none() {
        if !dry_run {
            return Err(CmdError::click(vast::vast_refusal(&reading))
                .stating(crate::primitives::failure::FailureCode::Config));
        }
        eprintln!("[vast] {}", vast::vast_refusal(&reading));
    }
    // The queue the bridge watches is the one the configuration selects,
    // as `monitor` reads it; a bucket written here would watch a queue the
    // operator never named.
    let store = JobStorage::with_bucket(crate::config::bucket()).await?;
    let hostname = vast::system_hostname();
    let params = AutoListParams {
        idle_window_s,
        poll: poll_interval_s.map(std::time::Duration::from_secs),
        price_gpu,
        price_disk,
        dry_run,
        once,
        duration_s: max_duration_s.is_positive().then_some(max_duration_s),
    };
    vast::auto_list_loop(client.as_ref(), &store, &hostname, params, |m| {
        println!("{m}")
    })
    .await
    .map_err(cmd_err)
}
