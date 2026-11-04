//! `stado cost host TARGET [--since HOURS] [--json]`: what each hour of an
//! owned host cost, from the hours its own beacon measured and the
//! electricity tariff the registry declares for it.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// Joules in one kilowatt-hour: 1000 W for 3600 s (SI).
const JOULES_PER_KWH: f64 = 3_600_000.0;

/// One hour as the report prints it.
fn priced(start: &str, hour: &Value, tariff: &crate::targets::PowerTariff) -> Value {
    let mut boards = serde_json::Map::new();
    let mut watt_seconds = 0.0;
    if let Some(measured) = hour["boards"].as_object() {
        for (uuid, board) in measured {
            let samples = board["samples"].as_u64().unwrap_or(0);
            let busy = board["busy_samples"].as_u64().unwrap_or(0);
            let energy = board["watt_seconds"].as_f64().unwrap_or(0.0);
            watt_seconds += energy;
            boards.insert(
                uuid.clone(),
                json!({
                    "samples": samples,
                    "busy_fraction": (samples > 0).then(|| busy as f64 / samples as f64),
                    "kwh": energy / JOULES_PER_KWH,
                }),
            );
        }
    }
    let cpu_samples = hour["cpu_samples"].as_u64().unwrap_or(0);
    let cpu_busy = (cpu_samples > 0)
        .then(|| hour["cpu_busy_sum"].as_f64().unwrap_or(0.0) / cpu_samples as f64);
    let kwh = watt_seconds / JOULES_PER_KWH;
    // A host whose boards were never sampled in this hour has no measured
    // energy: its cost is unknown, never zero.
    let measured = !boards.is_empty();
    json!({
        "hour": start,
        "boards": boards,
        "cpu_busy_fraction": cpu_busy,
        "kwh": measured.then_some(kwh),
        "cost": measured.then(|| kwh * tariff.price_per_kwh),
        "currency": tariff.currency,
        "unread": hour["unread"],
    })
}

pub(crate) async fn host(
    target: &str,
    since_hours: u32,
    json_output: bool,
) -> Result<(), CmdError> {
    let registry = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&registry)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let host = crate::cli::resolved_host(&registry, target)?;
    let tariff = host.power_tariff().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no power_tariff, so its hours have no price; declare one with `stado \
             registry set --path targets.{}.power_tariff --value '{{\"price_per_kwh\": <price>, \
             \"currency\": \"<code>\", \"source\": \"<the bill or published tariff it was read \
             from>\"}}'`",
            host.name, host.name
        ))
        .stating(FailureCode::Config)
    })?;
    let store = crate::queue::JobStorage::new().await?;
    let beacons = crate::cli::registry::beacon_bodies(&store).await?;
    let slugs = crate::monitor::host_health::beacon_slugs(host, &host.name);
    let beacon = slugs
        .iter()
        .find_map(|slug| beacons.get(slug))
        .ok_or_else(|| {
            CmdError::missing(format!(
                "{} has published no beacon under {}, so none of its hours were measured",
                host.name,
                slugs.join(", ")
            ))
        })?;
    let hours = beacon
        .get("hours")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::missing(format!(
                "{}'s beacon carries no hours block: its Stado predates host-hour measurement, or \
             its beacon is published once rather than by `stado serve --health-interval-seconds`",
                host.name
            ))
        })?;
    let since = (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours)))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows: Vec<Value> = hours
        .iter()
        .filter(|(start, _)| start.as_str() >= since.as_str())
        .map(|(start, hour)| priced(start, hour, &tariff))
        .collect();
    let total: f64 = rows.iter().filter_map(|row| row["cost"].as_f64()).sum();
    let unpriced: Vec<&str> = rows
        .iter()
        .filter(|row| row["cost"].is_null())
        .filter_map(|row| row["hour"].as_str())
        .collect();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.cost-host.v1",
                "host": host.name,
                "tariff": tariff,
                "since": since,
                "hours": rows,
                "total_cost": total,
                "unpriced_hours": unpriced,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} at {} {}/kWh ({}), since {since}",
        host.name, tariff.price_per_kwh, tariff.currency, tariff.source
    );
    for row in &rows {
        let boards = row["boards"]
            .as_object()
            .map(|boards| {
                boards
                    .iter()
                    .map(|(uuid, board)| {
                        format!(
                            "{uuid} busy {} {} kWh",
                            share(&board["busy_fraction"]),
                            number(&board["kwh"])
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        let cost = match row["cost"].as_f64() {
            Some(cost) => format!("{cost:.4} {}", tariff.currency),
            None => "unpriced: no board was sampled".to_string(),
        };
        println!(
            "{} cpu busy {}; {}; {cost}",
            row["hour"].as_str().unwrap_or("?"),
            share(&row["cpu_busy_fraction"]),
            if boards.is_empty() {
                "no boards".to_string()
            } else {
                boards
            }
        );
        if let Some(unread) = row["unread"].as_array().filter(|unread| !unread.is_empty()) {
            for cause in unread.iter().filter_map(Value::as_str) {
                println!("  unread: {cause}");
            }
        }
    }
    println!(
        "total {total:.4} {} over {} hour(s){}",
        tariff.currency,
        rows.len(),
        match unpriced.len() {
            0 => String::new(),
            n => format!(", {n} unpriced"),
        }
    );
    Ok(())
}

fn share(value: &Value) -> String {
    match value.as_f64() {
        Some(fraction) => format!("{:.0}%", fraction * 100.0),
        None => "unread".to_string(),
    }
}

fn number(value: &Value) -> String {
    match value.as_f64() {
        Some(number) => format!("{number:.3}"),
        None => "unread".to_string(),
    }
}
