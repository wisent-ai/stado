//! The Vast.ai adapter behind `--provider vast`, and the provider-less
//! market verbs (`sources`, `payout`, `compare`).
//!
//! `list`, `unlist`, `status`, `monitor`, the `follow` loop and the
//! `readiness` verdict, each against Vast.ai's host API. Monitor and
//! follow read the queue through [`JobStorage`], which honors
//! `WC_STORAGE_BACKEND`. A dry run builds no client: `--dry-run` promises the
//! toggle decisions without calling the marketplace, so it needs no
//! credential. The dispatch by `--provider` is [`super::market`].

mod compare;
#[path = "earnings.rs"]
mod earnings;
mod measure;
mod payout;
mod readiness;
mod readiness_text;
mod serve_or_rent;
mod sources;

pub(crate) use readiness::read_vault;
pub(crate) use sources::source_ids;

use serde_json::{json, Value};

use super::market::{Follow, ListOffer, MarketAdapter, ServeOrRent};
use crate::cli::{CmdError, MarketCommands, PayoutCommands};
use crate::providers::vast::{self, AutoListParams, ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

/// The verbs that name no marketplace, answered before any adapter is
/// chosen; `None` for every verb that names one.
pub(crate) async fn provider_less(command: &MarketCommands) -> Option<Result<(), CmdError>> {
    Some(match command {
        MarketCommands::Sources { check, json } => sources::sources(*check, *json).await,
        MarketCommands::Compare {
            target,
            since_days,
            json,
        } => compare::compare(target, *since_days, *json).await,
        MarketCommands::Decisions {
            target,
            since_hours,
            json,
        } => decisions(target, *since_hours, *json).await,
        MarketCommands::Payout(PayoutCommands::Record {
            source,
            host,
            paid_at,
            token,
            amount,
            usd_per_unit,
            as_of,
            rate_source,
            json,
        }) => {
            payout::record(
                source,
                host,
                paid_at,
                token,
                *amount,
                *usd_per_unit,
                as_of.as_deref(),
                rate_source.as_deref(),
                *json,
            )
            .await
        }
        MarketCommands::Payout(PayoutCommands::List { host, json }) => {
            payout::list(host.as_deref(), *json).await
        }
        _ => return None,
    })
}

/// `stado market decisions --target HOST`: the board decisions HOST's agent
/// kept, oldest first.
async fn decisions(target: &str, since_hours: u32, json_output: bool) -> Result<(), CmdError> {
    use crate::providers::local::agent::allocate;
    let store = JobStorage::new().await?;
    let since = (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours))).to_rfc3339();
    // A host's agent keeps its decisions under its worker name, the name
    // `--target` takes.
    let kept = allocate::kept(&store, target, &since).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.market-decisions.v1",
                "target": target,
                "decisions": kept,
            }))?
        );
        return Ok(());
    }
    if kept.is_empty() {
        println!("{target}'s agent kept no board decision in the last {since_hours} hour(s)");
        return Ok(());
    }
    for decision in &kept {
        let candidates: Vec<String> = decision
            .candidates
            .iter()
            .map(
                |candidate| match (&candidate.refused, candidate.usd_per_hour) {
                    (Some(why), _) => format!("{} refused: {why}", candidate.what.said()),
                    (None, Some(value)) => format!(
                        "{} {value:.4} USD/h ({})",
                        candidate.what.said(),
                        candidate.source
                    ),
                    (None, None) => format!("{} unvalued", candidate.what.said()),
                },
            )
            .collect();
        println!(
            "{} board of {}: {}; chose {}: {}",
            decision.at,
            decision.board,
            candidates.join("; "),
            decision
                .chosen
                .as_ref()
                .map_or("no change".to_string(), allocate::Use::said),
            decision.reason
        );
    }
    Ok(())
}

/// Vast.ai: the host is listed through its host API with the key in the
/// Skarbiec item `vast`, field `api_key`.
pub(crate) struct VastAdapter;

#[async_trait::async_trait]
impl MarketAdapter for VastAdapter {
    async fn list(&self, offer: ListOffer, json: bool) -> Result<(), CmdError> {
        let Some(price_disk) = offer.price_disk else {
            return Err(CmdError::usage(
                "listing on Vast.ai needs --price-disk, the per-GB-month disk price in USD",
            ));
        };
        let contract_hours = required_contract_hours(offer.contract_hours, "listing")?;
        list(
            offer.price_gpu,
            price_disk,
            offer.price_min_bid,
            contract_hours,
            json,
        )
        .await
    }

    async fn unlist(&self, json: bool) -> Result<(), CmdError> {
        unlist(json).await
    }

    async fn status(&self, json: bool) -> Result<(), CmdError> {
        status(json).await
    }

    async fn earnings(&self, target: &str, since_days: u32, json: bool) -> Result<(), CmdError> {
        earnings::earnings(target, since_days, json).await
    }

    async fn maintenance(
        &self,
        in_minutes: u32,
        hours: f64,
        category: &str,
        json: bool,
    ) -> Result<(), CmdError> {
        maintenance(in_minutes, hours, category, json).await
    }

    async fn measure(&self, gpu: &str, json: bool) -> Result<(), CmdError> {
        measure::measure(gpu, json).await
    }

    async fn serve_or_rent(&self, compared: ServeOrRent<'_>, json: bool) -> Result<(), CmdError> {
        serve_or_rent::serve_or_rent(
            compared.target,
            compared.outside_model,
            compared.gpu,
            compared.since_hours,
            json,
        )
        .await
    }

    async fn readiness(
        &self,
        vault_host: Option<String>,
        no_vault_check: bool,
        json: bool,
    ) -> Result<(), CmdError> {
        readiness::report(vault_host, no_vault_check, json).await
    }

    async fn verdicts(&self) -> Vec<Value> {
        let (ready, verdict) = readiness::verdict().await;
        vec![json!({"provider": "vast", "ready": ready, "verdict": verdict})]
    }

    async fn monitor(&self, bucket: &str, json: bool) -> Result<(), CmdError> {
        monitor(bucket, json).await
    }

    async fn follow(&self, follow: Follow) -> Result<(), CmdError> {
        let Some(price_disk) = follow.price_disk else {
            return Err(CmdError::usage(
                "following the queue on Vast.ai needs --price-disk, the per-GB-month disk price in USD",
            ));
        };
        let contract_hours = required_contract_hours(follow.contract_hours, "following")?;
        auto_list(
            follow.idle_window_s,
            follow.poll_interval_s,
            (follow.price_gpu, price_disk),
            follow.max_duration_s,
            contract_hours,
            follow.dry_run,
            follow.once,
        )
        .await
    }
}

/// The offer's contract window, which a Vast.ai offer always states.
fn required_contract_hours(hours: Option<u32>, verb: &str) -> Result<u32, CmdError> {
    hours.ok_or_else(|| {
        CmdError::usage(format!(
            "{verb} on Vast.ai needs --contract-hours: how many hours ahead the offer ends, so a \
             renter cannot hold the host past the window the fleet spares"
        ))
    })
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
    contract_hours: u32,
    json_output: bool,
) -> Result<(), CmdError> {
    if contract_hours == 0 {
        return Err(CmdError::usage(
            "--contract-hours is zero: the offer would end the moment it is listed; state how \
             many hours ahead it ends",
        ));
    }
    // Vast lists the machine whole, so it is listed only when a renter's
    // container would reach nothing of the host's and no board is held.
    if let Some(refusal) = vast::isolation::measure().await.refusal() {
        return Err(CmdError::refused(format!("not listed: {refusal}")));
    }
    if let Some(refusal) = vast::held::refusal().await {
        return Err(CmdError::refused(format!("not listed: {refusal}")));
    }
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let result = client
        .list_machine(&ListMachineParams {
            price_gpu,
            price_disk,
            price_min_bid,
            duration: None,
            end_date: ListMachineParams::end_in_hours(contract_hours),
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

/// Tell Vast this host goes down `in_minutes` from now for `hours`, so every
/// renter is notified, and print Vast's answer (how many it told).
async fn maintenance(
    in_minutes: u32,
    hours: f64,
    category: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let client = VastClient::from_env().await.map_err(cmd_err)?;
    let start = chrono::Utc::now().timestamp() + i64::from(in_minutes) * 60;
    let result = client
        .schedule_maintenance(start, hours, category)
        .await
        .map_err(cmd_err)?;
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
    contract_hours: u32,
    dry_run: bool,
    once: bool,
) -> Result<(), CmdError> {
    let reading = vast::read_vast_api_key().await;
    let client = reading.key.clone().map(VastClient::new);
    if client.is_none() {
        if !dry_run {
            return Err(
                CmdError::click(vast::vast_refusal(&reading)).stating(reading.failure_code())
            );
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
        contract_hours,
    };
    vast::auto_list_loop(client.as_ref(), &store, &hostname, params, |m| {
        println!("{m}")
    })
    .await
    .map_err(cmd_err)
}
