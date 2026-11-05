//! `stado demand event add|withdraw|paid|list|report`: grid demand-response
//! events the operator's chosen program announced, the hosts that shed
//! load for them, and what each event shed and paid.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::queue::demand::{self, Event, Payment};
use crate::queue::JobStorage;

#[derive(Subcommand)]
pub enum DemandCommands {
    /// Demand-response events: record one, withdraw it, record what it
    /// paid, list them, and report what each shed.
    #[command(subcommand)]
    Event(EventCommands),
}

#[derive(Subcommand)]
pub enum EventCommands {
    /// Record an event from the program's notice. During its window each
    /// named host claims nothing and yields its yieldable jobs back to the
    /// queue; work that does not yield keeps running.
    Add {
        id: String,
        /// The program, as the operator names it.
        #[arg(long)]
        program: String,
        /// A registry target that sheds load; repeat for each.
        #[arg(long = "host", required = true)]
        hosts: Vec<String>,
        #[arg(long = "starts-at")]
        starts_at: String,
        #[arg(long = "ends-at")]
        ends_at: String,
        /// Where the event was read (the program's notice).
        #[arg(long)]
        source: String,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw an event; its hosts claim again on their next tick.
    Withdraw {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Record what the program paid for an event.
    Paid {
        id: String,
        #[arg(long)]
        amount: f64,
        #[arg(long)]
        currency: String,
        /// Where the payment was read (the program's statement).
        #[arg(long)]
        source: String,
        #[arg(long)]
        json: bool,
    },
    /// Every recorded event, newest first.
    List {
        #[arg(long)]
        json: bool,
    },
    /// What one event shed: per host, the boards' energy in the window
    /// against the same length of time just before it, from the host's
    /// beacon hours, and the payment beside it.
    Report {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

async fn require(store: &JobStorage, id: &str) -> Result<Event, CmdError> {
    demand::load(store, id).await?.ok_or_else(|| {
        CmdError::missing(format!(
            "no demand event {id} is recorded; `stado demand event list` names them"
        ))
    })
}

fn print(event: &Event, verb: &str, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(event)?);
    } else {
        println!(
            "{verb} demand event {} ({}) on {} from {} to {}{}{}",
            event.id,
            event.program,
            event.hosts.join(", "),
            event.starts_at,
            event.ends_at,
            match &event.payment {
                Some(paid) => format!("; paid {} {} ({})", paid.amount, paid.currency, paid.source),
                None => String::new(),
            },
            match &event.withdrawn_at {
                Some(at) => format!("; withdrawn {at}"),
                None => String::new(),
            }
        );
    }
    Ok(())
}

/// Kilowatt-hours the boards of `host` drew in `[from, to)`, from its
/// beacon's hours, and the hours of that span its beacon does not hold.
fn energy(
    hours: Option<&serde_json::Map<String, Value>>,
    from: i64,
    to: i64,
) -> (f64, Vec<String>) {
    let mut kwh = 0.0;
    let mut missing = Vec::new();
    let mut at = from - from.rem_euclid(3600);
    while at < to {
        let start = chrono::DateTime::from_timestamp(at, 0)
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();
        match hours
            .and_then(|hours| hours.get(&start))
            .and_then(|hour| hour["boards"].as_object())
            .filter(|boards| !boards.is_empty())
        {
            Some(boards) => {
                kwh += boards
                    .values()
                    .filter_map(|board| board["watt_seconds"].as_f64())
                    .sum::<f64>()
                    / crate::deploy::host_hours::JOULES_PER_KWH;
            }
            None => missing.push(start),
        }
        at += 3600;
    }
    (kwh, missing)
}

pub async fn dispatch(command: &DemandCommands) -> Result<(), CmdError> {
    let DemandCommands::Event(command) = command;
    let store = JobStorage::new().await?;
    match command {
        EventCommands::Add {
            id,
            program,
            hosts,
            starts_at,
            ends_at,
            source,
            json,
        } => {
            if demand::load(&store, id).await?.is_some() {
                return Err(CmdError::refused(format!(
                    "demand event {id} is already recorded"
                )));
            }
            let document = crate::cli::registry::fetch_document().await?;
            let registry =
                crate::targets::load_registry_from_value(&document).map_err(|error| {
                    CmdError::declaration(format!("the registry does not load: {error}"))
                })?;
            for host in hosts {
                crate::cli::resolved_host(&registry, host)?;
            }
            let event = Event {
                id: id.clone(),
                program: program.clone(),
                hosts: hosts.clone(),
                starts_at: starts_at.clone(),
                ends_at: ends_at.clone(),
                source: source.clone(),
                recorded_at: chrono::Utc::now().to_rfc3339(),
                payment: None,
                withdrawn_at: None,
            };
            event.validate().map_err(CmdError::usage)?;
            demand::save(&store, &event).await?;
            print(&event, "recorded", *json)
        }
        EventCommands::Withdraw { id, json } => {
            let mut event = require(&store, id).await?;
            if event.withdrawn_at.is_some() {
                return Err(CmdError::refused(format!(
                    "demand event {id} is already withdrawn"
                )));
            }
            event.withdrawn_at = Some(chrono::Utc::now().to_rfc3339());
            demand::save(&store, &event).await?;
            print(&event, "withdrew", *json)
        }
        EventCommands::Paid {
            id,
            amount,
            currency,
            source,
            json,
        } => {
            if !(amount.is_finite() && *amount >= 0.0) {
                return Err(CmdError::usage(format!(
                    "--amount {amount} is not a payment"
                )));
            }
            if currency.trim().is_empty() || source.trim().is_empty() {
                return Err(CmdError::usage("--currency and --source cannot be empty"));
            }
            let mut event = require(&store, id).await?;
            event.payment = Some(Payment {
                amount: *amount,
                currency: currency.clone(),
                source: source.clone(),
            });
            demand::save(&store, &event).await?;
            print(&event, "recorded payment of", *json)
        }
        EventCommands::List { json } => {
            let events = demand::list(&store).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&events)?);
                return Ok(());
            }
            if events.is_empty() {
                println!("no demand event is recorded");
            }
            for event in &events {
                print(event, "event", false)?;
            }
            Ok(())
        }
        EventCommands::Report { id, json } => {
            let event = require(&store, id).await?;
            let (from, to) = match (
                chrono::DateTime::parse_from_rfc3339(&event.starts_at),
                chrono::DateTime::parse_from_rfc3339(&event.ends_at),
            ) {
                (Ok(from), Ok(to)) => (from.timestamp(), to.timestamp()),
                _ => {
                    return Err(CmdError::declaration(format!(
                        "{id}'s window does not read"
                    )))
                }
            };
            let document = crate::cli::registry::fetch_document().await?;
            let registry =
                crate::targets::load_registry_from_value(&document).map_err(|error| {
                    CmdError::declaration(format!("the registry does not load: {error}"))
                })?;
            let beacons = crate::cli::registry::beacon_bodies(&store).await?;
            let mut rows = Vec::new();
            for host in &event.hosts {
                let target = crate::cli::resolved_host(&registry, host)?;
                let slugs = crate::monitor::host_health::beacon_slugs(target, &target.name);
                let hours = slugs
                    .iter()
                    .find_map(|slug| beacons.get(slug))
                    .and_then(|beacon| beacon.get("hours"))
                    .and_then(Value::as_object);
                let (during, missing_during) = energy(hours, from, to);
                let (before, missing_before) = energy(hours, from - (to - from), from);
                let measured = missing_during.is_empty() && missing_before.is_empty();
                rows.push(json!({
                    "host": host,
                    "kwh_during": during,
                    "kwh_before": before,
                    "kwh_shed": measured.then(|| before - during),
                    "hours_unmeasured": missing_during.iter().chain(&missing_before).collect::<Vec<_>>(),
                }));
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "schema": "stado.demand-report.v1",
                        "event": event,
                        "hosts": rows,
                    }))?
                );
                return Ok(());
            }
            print(&event, "event", false)?;
            for row in &rows {
                match row["kwh_shed"].as_f64() {
                    Some(shed) => println!(
                        "  {}: {:.3} kWh during, {:.3} kWh before, {:.3} kWh shed",
                        row["host"].as_str().unwrap_or("?"),
                        row["kwh_during"].as_f64().unwrap_or(0.0),
                        row["kwh_before"].as_f64().unwrap_or(0.0),
                        shed
                    ),
                    None => println!(
                        "  {}: shed not measured: the beacon holds no reading for {} (it keeps one \
                         day; report within a day of the event)",
                        row["host"].as_str().unwrap_or("?"),
                        row["hours_unmeasured"]
                            .as_array()
                            .map(|hours| hours
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", "))
                            .unwrap_or_default()
                    ),
                }
            }
            Ok(())
        }
    }
}
