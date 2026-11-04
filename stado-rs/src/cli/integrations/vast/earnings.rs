//! `stado market earnings --provider vast --target HOST [--since-days N]`:
//! what a host's machine earned per day as Vast states it, kept in the fleet
//! store, beside what the same day of the host cost and the margin.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::providers::vast::{EarnedDay, VastClient, SECONDS_PER_DAY};

/// Where one stated day is kept.
fn ledger_key(host: &str, day: i64) -> String {
    format!("market/earnings/vast/{host}/{day}.json")
}

/// The UTC date of a day number.
fn date_of(day: i64) -> String {
    chrono::DateTime::from_timestamp(day * SECONDS_PER_DAY, 0)
        .map(|at| at.date_naive().to_string())
        .unwrap_or_else(|| format!("day {day}"))
}

/// What the host's day cost, from the hours its beacon measured: the energy
/// its boards drew above their idle draw in each of the day's 24 hours, at
/// the tariff in US dollars, as `stado cost host` and the scheduler charge
/// it. Every hour must be measured and every board's idle draw known, else
/// the reason it is not.
fn day_cost(
    hours: Option<&serde_json::Map<String, Value>>,
    tariff: &Option<crate::targets::PowerTariff>,
    day: i64,
) -> Result<f64, String> {
    let tariff = tariff.as_ref().ok_or("the host declares no power_tariff")?;
    // Vast states US dollars; the tariff is converted at its declared rate.
    let usd_per_kwh = tariff.usd_per_kwh()?;
    let hours = hours.ok_or("the host's beacon carries no measured hours")?;
    let idle = crate::deploy::host_hours::idle_watts(hours);
    let mut kwh = 0.0;
    for hour in 0..24 {
        let start = chrono::DateTime::from_timestamp(day * SECONDS_PER_DAY + hour * 3600, 0)
            .ok_or("the day cannot be written as a time")?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let measured = hours
            .get(&start)
            .and_then(|bucket| bucket["boards"].as_object())
            .filter(|boards| !boards.is_empty())
            .ok_or_else(|| format!("hour {start} was not measured (the beacon keeps one day)"))?;
        for (uuid, board) in measured {
            kwh +=
                crate::deploy::host_hours::kwh_above_idle(uuid, board, &idle).ok_or_else(|| {
                    format!("board {uuid} has no measured idle draw, so hour {start} is unpriced")
                })?;
        }
    }
    Ok(kwh * usd_per_kwh)
}

pub(crate) async fn earnings(
    target: &str,
    since_days: u32,
    json_output: bool,
) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let host = crate::cli::resolved_host(&registry, target)?;
    let client = VastClient::from_env().await.map_err(|error| {
        CmdError::click(format!(
            "Vast.ai cannot be asked what {} earned: {error}",
            host.name
        ))
        .stating(FailureCode::Auth)
    })?;
    // The machine is the one Vast lists under the host's own hostname, as
    // every other market operation of this host resolves it.
    let hostname = host
        .hostnames
        .first()
        .map(String::as_str)
        .unwrap_or(&host.name);
    let machine = client
        .machine_id_for_hostname(hostname)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{}'s Vast.ai machine cannot be named: {error}",
                host.name
            ))
            .stating(FailureCode::NotFound)
        })?;
    let today = chrono::Utc::now().timestamp() / SECONDS_PER_DAY;
    let since = today - i64::from(since_days);
    let stated = client
        .machine_earnings(machine, since, today)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "Vast.ai did not state {}'s earnings: {error}",
                host.name
            ))
            .stating(FailureCode::InfraDown)
        })?;
    // Every stated day is kept, create-or-replace: Vast restates a day while
    // it is still accruing, and the newest statement is the day's.
    let store = crate::queue::JobStorage::new().await?;
    let read_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for day in &stated {
        let record =
            json!({"machine_id": machine, "read_at": read_at, "day": day, "gross": day.gross()});
        store
            .upload_bytes(
                &ledger_key(&host.name, day.day),
                serde_json::to_vec(&record)?.as_slice(),
            )
            .await
            .map_err(|error| {
                CmdError::from(error).within(format!(
                    "keep {}'s earnings of {}",
                    host.name,
                    date_of(day.day)
                ))
            })?;
    }
    let beacons = crate::cli::registry::beacon_bodies(&store).await?;
    let slugs = crate::monitor::host_health::beacon_slugs(host, &host.name);
    let hours = slugs
        .iter()
        .find_map(|slug| beacons.get(slug))
        .and_then(|beacon| beacon.get("hours"))
        .and_then(Value::as_object);
    let tariff = host.power_tariff();
    // The same host's payouts from other sources in a token: each is the
    // income of the UTC day it was paid, valued at the rate kept with it.
    let payouts = super::payout::kept(&store, Some(&host.name)).await?;
    let rows: Vec<Value> = (since..=today)
        .map(|day| {
            let earned: Option<&EarnedDay> = stated.iter().find(|row| row.day == day);
            let cost = day_cost(hours, &tariff, day);
            let paid: Vec<&crate::providers::payout::TokenPayout> = payouts
                .iter()
                .filter(|payout| {
                    chrono::DateTime::parse_from_rfc3339(&payout.paid_at)
                        .is_ok_and(|at| at.timestamp().div_euclid(SECONDS_PER_DAY) == day)
                })
                .collect();
            let tokens_usd: f64 = paid.iter().filter_map(|payout| payout.usd().ok()).sum();
            let unpriced: Vec<String> = paid
                .iter()
                .filter_map(|payout| {
                    payout.usd().err().map(|cause| {
                        format!(
                            "{} {} from {}: {cause}",
                            payout.amount, payout.token, payout.source
                        )
                    })
                })
                .collect();
            json!({
                "date": date_of(day),
                "day": day,
                "earned": earned,
                "gross": earned.map(EarnedDay::gross),
                "payouts": paid,
                "payouts_usd": tokens_usd,
                "payouts_unpriced": unpriced,
                "cost": cost.as_ref().ok(),
                "cost_unread": cost.as_ref().err(),
                // A day with an unpriced payout has no margin: its income is
                // not known, and leaving the payout out would understate it.
                "margin": match (earned, &cost) {
                    (Some(earned), Ok(cost)) if unpriced.is_empty() => {
                        Some(earned.gross() + tokens_usd - cost)
                    }
                    _ => None,
                },
            })
        })
        .collect();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.market-earnings.v1",
                "provider": "vast",
                "host": host.name,
                "machine_id": machine,
                "days": rows,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} (Vast.ai machine {machine}), USD gross as Vast states it:",
        host.name
    );
    for row in &rows {
        let gross = match row["gross"].as_f64() {
            Some(gross) => format!("earned {gross:.2}"),
            None => "earnings not stated by Vast".to_string(),
        };
        let tokens = match row["payouts"].as_array().map(Vec::len) {
            Some(0) | None => String::new(),
            Some(count) => format!(
                "; {count} token payout(s) worth {:.2}",
                row["payouts_usd"].as_f64().unwrap_or(0.0)
            ),
        };
        let unpriced: Vec<&str> = row["payouts_unpriced"]
            .as_array()
            .map(|causes| causes.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let unpriced = if unpriced.is_empty() {
            String::new()
        } else {
            format!("; unpriced: {}", unpriced.join("; "))
        };
        let cost = match (row["cost"].as_f64(), row["cost_unread"].as_str()) {
            (Some(cost), _) => format!("cost {cost:.2}"),
            (None, Some(cause)) => format!("cost unread: {cause}"),
            (None, None) => "cost unread".to_string(),
        };
        let margin = match row["margin"].as_f64() {
            Some(margin) => format!("; margin {margin:.2}"),
            None => String::new(),
        };
        println!(
            "{} {gross}{tokens}{unpriced}; {cost}{margin}",
            row["date"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}
