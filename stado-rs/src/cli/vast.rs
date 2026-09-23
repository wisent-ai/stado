//! `stado vast` command group.
//!
//! Port of the `vast` group in `stado/cli.py`: `list`, `unlist`, `status`,
//! `monitor`, and the `auto-list` daemon. Output is `json.dumps(...,
//! indent=2)` like the Python click commands (2-space pretty JSON with
//! ensure_ascii escaping).
//!
//! Deviation: Python's monitor/auto-list probes always construct a GCS
//! client; the Rust port routes through [`JobStorage`], which honors
//! `WC_STORAGE_BACKEND` — on the lab box (the only place this runs) the
//! backend is gcs either way.

use serde_json::{json, Value};

use super::{CmdError, VastCommands};
use crate::providers::vast::{self, AutoListParams, ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

/// Dispatch one `vast` subcommand.
pub(super) async fn dispatch(command: &VastCommands) -> Result<(), CmdError> {
    match command {
        VastCommands::List {
            price_gpu,
            price_disk,
            price_min_bid,
        } => list(*price_gpu, *price_disk, *price_min_bid).await,
        VastCommands::Unlist => unlist().await,
        VastCommands::Status => status().await,
        VastCommands::Monitor { bucket } => monitor(bucket).await,
        VastCommands::AutoList {
            idle_window_s,
            poll_interval_s,
            price_gpu,
            max_duration_s,
            dry_run,
        } => {
            auto_list(
                *idle_window_s,
                *poll_interval_s,
                *price_gpu,
                *max_duration_s,
                *dry_run,
            )
            .await
        }
    }
}

/// Any bridge failure surfaces as a click-style `Error: {msg}` (exit 1).
/// Python raises ClickException for VastConfigError and lets RuntimeError
/// tracebacks exit 1 — both land here.
fn cmd_err(exc: VastError) -> CmdError {
    CmdError::click(exc.to_string())
}

/// Python `click.echo(json.dumps(payload, indent=2, default=str))`.
fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

async fn list(price_gpu: f64, price_disk: f64, price_min_bid: Option<f64>) -> Result<(), CmdError> {
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
    echo_json(&result);
    Ok(())
}

async fn unlist() -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client.unlist_machine().await.map_err(cmd_err)?;
    echo_json(&result);
    Ok(())
}

async fn status() -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client.machine_status().await.map_err(cmd_err)?;
    echo_json(&result);
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

async fn monitor(bucket: &str) -> Result<(), CmdError> {
    // Python catches VastConfigError into an {"error": ...} record; other
    // exceptions propagate.
    let vast_machine = match VastClient::from_env().await {
        Err(VastError::Config(message)) => json!({"error": message}),
        Err(exc) => return Err(cmd_err(exc)),
        Ok(client) => match client.machine_status().await {
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
    echo_json(&json!({
        "now": now_utc_iso_z(),
        "hostname": hostname,
        "vast_machine": vast_machine,
        "wisent_capacity": capacity,
        "wisent_queue": queued,
        "wisent_running": running,
    }));
    Ok(())
}

async fn auto_list(
    idle_window_s: i64,
    poll_interval_s: i64,
    price_gpu: f64,
    max_duration_s: i64,
    dry_run: bool,
) -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    // Python auto_list_loop's default bucket is the literal
    // "wisent-compute" (the CLI exposes no --bucket here).
    let store = JobStorage::with_bucket("wisent-compute").await?;
    let hostname = vast::system_hostname();
    let params = AutoListParams {
        idle_window_s,
        poll_interval_s: poll_interval_s.max(0) as u64,
        price_gpu,
        duration_s: if max_duration_s > 0 {
            Some(max_duration_s)
        } else {
            None
        },
        dry_run,
    };
    vast::auto_list_loop(&client, &store, &hostname, params, |m| println!("{m}"))
        .await
        .map_err(cmd_err)
}
