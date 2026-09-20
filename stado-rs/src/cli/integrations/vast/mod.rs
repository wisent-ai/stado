//! `stado vast` command group.
//!
//! Port of the `vast` group in `stado/cli.py`: `list`, `unlist`, `status`,
//! `monitor`, and the `auto-list` daemon, plus the `readiness` verdict this
//! port added. Output is `json.dumps(..., indent=2)` like the Python click
//! commands (2-space pretty JSON with ensure_ascii escaping).
//!
//! Deviation: Python's monitor/auto-list probes always construct a GCS
//! client; the Rust port routes through [`JobStorage`], which honors
//! `WC_STORAGE_BACKEND` — on the lab box (the only place this runs) the
//! backend is gcs either way.
//!
//! Second deviation, 2026-09-20: a dry run builds no client. `--dry-run`
//! promises the toggle decisions without calling the Vast API, and it refused
//! without `stado-vast/api_key` — so the flag could not be used on the
//! machine an operator reaches for it on, and the decision loop was
//! unobservable everywhere it was not already provisioned.

mod readiness;

use serde_json::{json, Value};

use crate::cli::{CmdError, VastCommands};
use crate::providers::vast::{self, AutoListParams, ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

/// Dispatch one `vast` subcommand.
pub(crate) async fn dispatch(command: &VastCommands) -> Result<(), CmdError> {
    match command {
        VastCommands::List {
            price_gpu,
            price_disk,
            price_min_bid,
        } => list(*price_gpu, *price_disk, *price_min_bid).await,
        VastCommands::Unlist => unlist().await,
        VastCommands::Status => status().await,
        VastCommands::Readiness {
            vault_host,
            no_vault_check,
            json,
        } => readiness::report(vault_host.clone(), *no_vault_check, *json).await,
        VastCommands::Monitor { bucket } => monitor(bucket).await,
        VastCommands::AutoList {
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

/// One snapshot: what Vast says about our machine, what the queue holds, and
/// — since 2026-09-20 — which Skarbiec channel the credential came through.
///
/// The credential block is here because the snapshot's `vast_machine.error`
/// was the only signal an operator had, and it named neither the consumer
/// that asked nor the channel it asked on.
async fn monitor(bucket: &str) -> Result<(), CmdError> {
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
    echo_json(&json!({
        "now": now_utc_iso_z(),
        "hostname": hostname,
        "credential": serde_json::to_value(&reading)?,
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
        poll_interval_s: poll_interval_s.max(0) as u64,
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
