//! `stado cost host TARGET [--since HOURS] [--json]`: what each hour of an
//! owned host cost, from the hours its own beacon measured and the
//! electricity tariff the registry declares for it.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use crate::deploy::host_hours::JOULES_PER_KWH;

/// One hour as the report prints it. `idle` is each board's idle draw over
/// the window ([`crate::deploy::host_hours::idle_watts`]): the energy above
/// it is what work on the board added, and a board whose idle draw was never
/// measured has that part unread.
fn priced(
    start: &str,
    hour: &Value,
    tariff: &crate::targets::PowerTariff,
    idle: &std::collections::BTreeMap<String, f64>,
) -> Value {
    let mut boards = serde_json::Map::new();
    let mut watt_seconds = 0.0;
    let mut above_idle = Some(0.0);
    if let Some(measured) = hour["boards"].as_object() {
        for (uuid, board) in measured {
            let samples = board["samples"].as_u64().unwrap_or(0);
            let busy = board["busy_samples"].as_u64().unwrap_or(0);
            let energy = board["watt_seconds"].as_f64().unwrap_or(0.0);
            watt_seconds += energy;
            let above = crate::deploy::host_hours::kwh_above_idle(uuid, board, idle)
                .map(|kwh| kwh * JOULES_PER_KWH);
            above_idle = above_idle.zip(above).map(|(sum, above)| sum + above);
            boards.insert(
                uuid.clone(),
                json!({
                    "samples": samples,
                    "busy_fraction": (samples > 0).then(|| busy as f64 / samples as f64),
                    "kwh": energy / JOULES_PER_KWH,
                    "idle_watts": idle.get(uuid),
                    "kwh_above_idle": above.map(|above| above / JOULES_PER_KWH),
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
    let above_kwh = above_idle
        .filter(|_| measured)
        .map(|above| above / JOULES_PER_KWH);
    json!({
        "hour": start,
        "boards": boards,
        "cpu_busy_fraction": cpu_busy,
        "kwh": measured.then_some(kwh),
        "cost": measured.then(|| kwh * tariff.price_per_kwh),
        "kwh_above_idle": above_kwh,
        "cost_above_idle": above_kwh.map(|above| above * tariff.price_per_kwh),
        "currency": tariff.currency,
        "cost_usd": tariff.usd_per_kwh().ok().filter(|_| measured).map(|usd| kwh * usd),
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
    let idle = crate::deploy::host_hours::idle_watts(hours);
    let rows: Vec<Value> = hours
        .iter()
        .filter(|(start, _)| start.as_str() >= since.as_str())
        .map(|(start, hour)| priced(start, hour, &tariff, &idle))
        .collect();
    let total: f64 = rows.iter().filter_map(|row| row["cost"].as_f64()).sum();
    let total_above_idle: f64 = rows
        .iter()
        .filter_map(|row| row["cost_above_idle"].as_f64())
        .sum();
    // The tariff in US dollars, which marketplaces and the cost report state;
    // a currency with no declared rate says so instead of a converted total.
    let usd = tariff.usd_per_kwh();
    let total_usd = usd.as_ref().ok().map(|_| {
        rows.iter()
            .filter_map(|row| row["cost_usd"].as_f64())
            .sum::<f64>()
    });
    // Wear: the declared purchase price spread over the declared service
    // life, carried by every hour of the window whether busy or idle.
    let hardware = host.hardware_cost();
    let wear = hardware
        .as_ref()
        .map(|hardware| hardware.per_hour() * rows.len() as f64);
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
                "total_cost_above_idle": total_above_idle,
                "total_cost_usd": total_usd,
                "usd_unread": usd.as_ref().err(),
                "hardware": hardware,
                "wear": wear,
                "unpriced_hours": unpriced,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} at {} {}/kWh ({}), since {since}",
        host.name, tariff.price_per_kwh, tariff.currency, tariff.source
    );
    match (&usd, &tariff.usd_rate) {
        (Ok(usd), Some(rate)) => println!(
            "  in USD: {usd:.4}/kWh at {} USD per {} as of {} ({})",
            rate.usd_per_unit, tariff.currency, rate.as_of, rate.source
        ),
        (Ok(_), None) => {}
        (Err(cause), _) => println!("  in USD: unpriced: {cause}"),
    }
    match &hardware {
        Some(hardware) => println!(
            "  wear: {:.4} {}/hour ({} {} over {} years, {})",
            hardware.per_hour(),
            hardware.currency,
            hardware.purchase_price,
            hardware.currency,
            hardware.service_years,
            hardware.source
        ),
        None => println!(
            "  wear: not counted: {} declares no hardware (purchase_price, currency, \
             service_years, source)",
            host.name
        ),
    }
    for row in &rows {
        let boards = row["boards"]
            .as_object()
            .map(|boards| {
                boards
                    .iter()
                    .map(|(uuid, board)| {
                        format!(
                            "{uuid} busy {} {} kWh, {} above idle {} W",
                            share(&board["busy_fraction"]),
                            number(&board["kwh"]),
                            number(&board["kwh_above_idle"]),
                            number(&board["idle_watts"])
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        let cost = match (row["cost"].as_f64(), row["cost_above_idle"].as_f64()) {
            (Some(cost), Some(above)) => format!(
                "{cost:.4} {currency}, {above:.4} {currency} above idle",
                currency = tariff.currency
            ),
            (Some(cost), None) => format!(
                "{cost:.4} {}, above idle unread: a board's idle draw was never measured",
                tariff.currency
            ),
            (None, _) => "unpriced: no board was sampled".to_string(),
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
        "total {total:.4} {currency}, {total_above_idle:.4} {currency} above idle, over {} \
         hour(s){}",
        rows.len(),
        match unpriced.len() {
            0 => String::new(),
            n => format!(", {n} unpriced"),
        },
        currency = tariff.currency,
    );
    if let Some(total_usd) = total_usd {
        println!("total in USD {total_usd:.4}");
    }
    if let (Some(wear), Some(hardware)) = (wear, &hardware) {
        println!(
            "wear over {} hour(s) {wear:.4} {}",
            rows.len(),
            hardware.currency
        );
    }
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
