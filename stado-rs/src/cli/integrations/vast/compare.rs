//! `stado market compare --target HOST [--since-days N] [--json]`: what each
//! of a host's uses earned per day beside what the day cost, from what Stado
//! already keeps, so the uses are compared on measured figures.
//!
//! The rows: Vast rentals (the days `stado market earnings` kept from Vast's
//! statement), inference sold to outside callers (the gateway's kept usage at
//! each caller's price), and token payouts by source (`stado market payout
//! record`: a network node's, a proving or mining workload's, any source
//! paid in a token). Each use is priced only from its own statement; a use
//! with no figure for a day is absent that day, never zero. The day's energy
//! cost (above idle, at the tariff band of each hour) is the same for every
//! use and is printed once; each use's margin is its income less that cost,
//! read as "what the day would have cleared had this been the board's use".
//! Nothing is asked of a marketplace: run `stado market earnings` first to
//! keep Vast's statement.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::providers::vast::SECONDS_PER_DAY;

use super::earnings::{date_of, day_cost, ledger_key, sold_by_day};

pub(crate) async fn compare(
    target: &str,
    since_days: u32,
    json_output: bool,
) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let host = crate::cli::resolved_host(&registry, target)?;
    let store = crate::queue::JobStorage::new().await?;
    let today = chrono::Utc::now().timestamp() / SECONDS_PER_DAY;
    let since = today - i64::from(since_days);

    let beacons = crate::cli::registry::beacon_bodies(&store).await?;
    let slugs = crate::monitor::host_health::beacon_slugs(host, &host.name);
    let hours = slugs
        .iter()
        .find_map(|slug| beacons.get(slug))
        .and_then(|beacon| beacon.get("hours"))
        .and_then(Value::as_object);
    let tariff = host.power_tariff();
    let sold = sold_by_day(&store, &document, &host.name, since).await?;
    let payouts = super::payout::kept(&store, Some(&host.name)).await?;

    let mut days = Vec::new();
    for day in since..=today {
        let mut uses: BTreeMap<String, Value> = BTreeMap::new();
        // Vast: the day's kept statement, when one was kept.
        if let Some(text) = store.download_text(&ledger_key(&host.name, day)).await? {
            let record: Value = serde_json::from_str(&text).map_err(|error| {
                CmdError::declaration(format!(
                    "{} is not a kept earnings day: {error}",
                    ledger_key(&host.name, day)
                ))
            })?;
            if let Some(gross) = record["gross"].as_f64() {
                uses.insert("vast rental".into(), json!({"usd": gross}));
            }
        }
        // Inference sold to outside callers.
        if let Some((usd, unpriced)) = sold.get(&day) {
            uses.insert(
                "inference sold".into(),
                json!({"usd": unpriced.is_empty().then_some(*usd), "unpriced": unpriced}),
            );
        }
        // Token payouts, one use per source.
        for payout in payouts.iter().filter(|payout| {
            chrono::DateTime::parse_from_rfc3339(&payout.paid_at)
                .is_ok_and(|at| at.timestamp().div_euclid(SECONDS_PER_DAY) == day)
        }) {
            let entry = uses
                .entry(format!("{} payouts", payout.source))
                .or_insert_with(|| json!({"usd": 0.0, "unpriced": []}));
            match payout.usd() {
                Ok(usd) => {
                    let total = entry["usd"].as_f64().unwrap_or(0.0) + usd;
                    entry["usd"] = json!(total);
                }
                Err(cause) => {
                    if let Some(list) = entry["unpriced"].as_array_mut() {
                        list.push(json!(format!(
                            "{} {}: {cause}",
                            payout.amount, payout.token
                        )));
                    }
                }
            }
        }
        let cost = day_cost(hours, &tariff, day);
        for use_row in uses.values_mut() {
            let priced = use_row["usd"].as_f64().filter(|_| {
                use_row["unpriced"]
                    .as_array()
                    .is_none_or(|unpriced| unpriced.is_empty())
            });
            use_row["margin"] = match (priced, &cost) {
                (Some(usd), Ok(cost)) => json!(usd - cost),
                _ => Value::Null,
            };
        }
        days.push(json!({
            "date": date_of(day),
            "cost": cost.as_ref().ok(),
            "cost_unread": cost.as_ref().err(),
            "uses": uses,
        }));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.market-compare.v1",
                "host": host.name,
                "days": days,
            }))?
        );
        return Ok(());
    }
    println!(
        "{}: each use's income per day beside the day's energy cost, in USD",
        host.name
    );
    for day in &days {
        let cost = match (day["cost"].as_f64(), day["cost_unread"].as_str()) {
            (Some(cost), _) => format!("cost {cost:.2}"),
            (None, Some(cause)) => format!("cost unread: {cause}"),
            _ => "cost unread".to_string(),
        };
        let uses = day["uses"].as_object().filter(|uses| !uses.is_empty());
        let Some(uses) = uses else {
            println!(
                "{} no use stated income; {cost}",
                day["date"].as_str().unwrap_or("?")
            );
            continue;
        };
        let rows: Vec<String> = uses
            .iter()
            .map(|(name, row)| {
                let income = match row["usd"].as_f64() {
                    Some(usd) => format!("{usd:.2}"),
                    None => "unpriced".to_string(),
                };
                let margin = match row["margin"].as_f64() {
                    Some(margin) => format!(" (margin {margin:.2})"),
                    None => String::new(),
                };
                format!("{name} {income}{margin}")
            })
            .collect();
        println!(
            "{} {}; {cost}",
            day["date"].as_str().unwrap_or("?"),
            rows.join(", ")
        );
    }
    Ok(())
}
