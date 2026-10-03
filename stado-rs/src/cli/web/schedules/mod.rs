//! `stado web schedule` — the requests a web product's unit is sent on a
//! cron, declared, withdrawn and read.
//!
//! The declaration is written here; the fleet schedules that send the
//! requests are made by `stado web route`, in the same pass that moves the
//! hostname to this fleet, and deleted by `stado web remove`. A product still
//! served by another platform therefore keeps its schedules there until its
//! cutover, and is never called by both.

mod command;
mod fleet;

use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::{mutate_web, product};
use crate::cli::CmdError;
use crate::queue::JobStorage;

pub(crate) use fleet::{activate, withdraw};

#[derive(Debug, Subcommand)]
pub(crate) enum ScheduleCommands {
    /// Declare or change one scheduled request of a web product.
    Set {
        /// Web product name.
        product: String,
        /// Schedule name, canonical (lowercase letters, digits, dashes).
        name: String,
        /// Request path on the unit, starting with `/`.
        #[arg(long)]
        path: String,
        /// HTTP method, upper case.
        #[arg(long)]
        method: String,
        /// 5-field cron expression.
        #[arg(long)]
        cron: String,
        /// IANA time zone the cron is read in.
        #[arg(long)]
        tz: String,
        /// Header that carries the secret, e.g. `authorization`.
        #[arg(long = "secret-header", requires = "secret")]
        secret_header: Option<String>,
        /// `role#field` whose value the header carries.
        #[arg(long, requires = "secret_header")]
        secret: Option<String>,
        /// Scheme written before the value, e.g. `Bearer`.
        #[arg(long = "secret-scheme", requires = "secret")]
        secret_scheme: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw one scheduled request from a web product's declaration.
    Remove {
        /// Web product name.
        product: String,
        /// Schedule name.
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// List declared scheduled requests and the fleet schedules sending them.
    List {
        /// Web product name; omit for every declared product.
        product: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn dispatch(command: ScheduleCommands) -> Result<(), CmdError> {
    match command {
        ScheduleCommands::Set {
            product: name,
            name: schedule,
            path,
            method,
            cron,
            tz,
            secret_header,
            secret,
            secret_scheme,
            json,
        } => {
            let mut entry = Map::new();
            entry.insert("path".into(), json!(path));
            entry.insert("method".into(), json!(method));
            entry.insert("cron".into(), json!(cron));
            entry.insert("tz".into(), json!(tz));
            if let (Some(header), Some(value)) = (secret_header, secret) {
                let mut carried = Map::new();
                carried.insert("header".into(), json!(header));
                carried.insert("value".into(), json!(value));
                if let Some(scheme) = secret_scheme {
                    carried.insert("scheme".into(), json!(scheme));
                }
                entry.insert("secret".into(), Value::Object(carried));
            }
            write(&name, &schedule, Some(Value::Object(entry)), json)
        }
        ScheduleCommands::Remove {
            product: name,
            name: schedule,
            json,
        } => write(&name, &schedule, None, json),
        ScheduleCommands::List { product, json } => list(product.as_deref(), json).await,
    }
}

/// Put or take one schedule in the product's declaration. The configuration
/// parser judges the result, so a refused value leaves the file unchanged.
fn write(
    name: &str,
    schedule: &str,
    entry: Option<Value>,
    json_output: bool,
) -> Result<(), CmdError> {
    product(name)?;
    let change = std::cell::Cell::new("unchanged");
    mutate_web("products", |products| {
        let declaration = products
            .get_mut(name)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("web_api.products.{name} is not an object"))?;
        let schedules = declaration
            .entry("schedules".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| format!("web_api.products.{name}.schedules must be an object"))?;
        match entry {
            Some(entry) => {
                let previous = schedules.insert(schedule.to_string(), entry.clone());
                change.set(match previous {
                    None => "declared",
                    Some(previous) if previous == entry => "unchanged",
                    Some(_) => "changed",
                });
            }
            None => {
                if schedules.remove(schedule).is_none() {
                    return Err(format!(
                        "web product {name} declares no schedule {schedule:?}"
                    ));
                }
                change.set("withdrawn");
            }
        }
        if schedules.is_empty() {
            declaration.remove("schedules");
        }
        Ok(())
    })?;
    let report = json!({"product": name, "schedule": schedule, "change": change.get()});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name}: schedule {schedule} {}; `stado web route {name}` makes the fleet send it",
            change.get()
        );
    }
    Ok(())
}

/// Each declared schedule beside the fleet schedule that sends it, if the
/// product has been routed: enabled or not, the next run, the last run and
/// the job that last fired.
async fn list(only: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let products = crate::config::web_api_products()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    if let Some(name) = only {
        product(name)?;
    }
    let store = JobStorage::new().await?;
    let mut rows = Vec::new();
    for (name, declared) in products
        .iter()
        .filter(|(name, _)| only.is_none_or(|only| only == name.as_str()))
    {
        let fleet = fleet::owned(&store, name).await?;
        for (schedule_name, schedule) in declared.schedules() {
            let prefix = format!("sch-web.{name}.{schedule_name}.");
            let sending = fleet
                .iter()
                .find(|item| item.schedule_id.starts_with(&prefix));
            rows.push(json!({
                "product": name,
                "schedule": schedule_name,
                "method": schedule.method(),
                "path": schedule.path(),
                "cron": schedule.cron(),
                "tz": schedule.tz(),
                "fleet_schedule": sending.map(|item| json!({
                    "id": item.schedule_id,
                    "enabled": item.enabled,
                    "next_due_at": item.next_due_at,
                    "last_fired_at": item.last_fired_at,
                    "last_job_id": item.last_job_id,
                })),
            }));
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no web product declares a schedule");
    }
    for row in &rows {
        let state = match &row["fleet_schedule"] {
            Value::Null => "not sent yet: the product has not been routed".to_string(),
            sending => format!(
                "{} {}, next {}, last job {}",
                sending["id"].as_str().unwrap_or_default(),
                if sending["enabled"].as_bool() == Some(true) {
                    "enabled"
                } else {
                    "paused"
                },
                sending["next_due_at"].as_str().unwrap_or_default(),
                sending["last_job_id"]
                    .as_str()
                    .filter(|job| !job.is_empty())
                    .unwrap_or("none"),
            ),
        };
        println!(
            "{}/{}: {} {} at {} ({}) — {state}",
            row["product"].as_str().unwrap_or_default(),
            row["schedule"].as_str().unwrap_or_default(),
            row["method"].as_str().unwrap_or_default(),
            row["path"].as_str().unwrap_or_default(),
            row["cron"].as_str().unwrap_or_default(),
            row["tz"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}
