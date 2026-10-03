//! A web product's declared schedules made real as fleet schedules, and
//! withdrawn again.
//!
//! A fleet schedule a web product owns is found by its id, which this module
//! alone mints: `sch-web.<product>.<schedule>.<8 hex>`. Product and schedule
//! names are canonical (lowercase letters, digits, dashes), so the dots
//! separate them unambiguously. A deleted schedule stays as a tombstone under
//! its id, so a changed declaration is a new schedule with a new id, never an
//! old one revived.

use chrono::Utc;
use serde_json::{json, Value};

use super::command::{request_command, secret_env};
use crate::cli::CmdError;
use crate::config::{WebApiProduct, WebApiSchedule};
use crate::queue::JobStorage;
use crate::schedules::{self, compute_next_due, Schedule};

/// The id prefix of every fleet schedule `product` owns.
fn product_prefix(product: &str) -> String {
    format!("sch-web.{product}.")
}

/// The id prefix of the fleet schedule one declared schedule became.
fn schedule_prefix(product: &str, schedule: &str) -> String {
    format!("{}{schedule}.", product_prefix(product))
}

fn minted_id(product: &str, schedule: &str) -> String {
    format!(
        "{}{}",
        schedule_prefix(product, schedule),
        hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4])
    )
}

/// Every live fleet schedule `product` owns.
pub(super) async fn owned(store: &JobStorage, product: &str) -> Result<Vec<Schedule>, CmdError> {
    let prefix = product_prefix(product);
    Ok(schedules::list_schedules(store)
        .await?
        .into_iter()
        .filter(|schedule| schedule.schedule_id.starts_with(&prefix))
        .collect())
}

/// The fleet schedule one declaration describes, before it is written.
async fn rendered(
    name: &str,
    declared: &WebApiProduct,
    schedule_name: &str,
    schedule: &WebApiSchedule,
) -> Result<Schedule, CmdError> {
    let mut fleet = Schedule::new(
        minted_id(name, schedule_name),
        schedule.cron(),
        request_command(declared, schedule),
    );
    fleet.tz = schedule.tz().to_string();
    // Stado chooses the provider; the pinned host is what routes the job.
    fleet.provider = String::new();
    fleet.repo_extras = String::new();
    fleet.pinned_host = crate::cli::submit::resolve_pinned_host(declared.host()).await?;
    fleet.secret_env = crate::cli::submit::parse_secret_env(&secret_env(schedule))?;
    fleet.overlap_policy = "skip".to_string();
    fleet.created_by = format!("stado web route {name}");
    let next = compute_next_due(&fleet.cron, Utc::now(), &fleet.tz).map_err(|error| {
        CmdError::click(format!(
            "web_api.products.{name}.schedules.{schedule_name}: no next run in {}: {error}",
            fleet.tz
        ))
    })?;
    fleet.next_due_at = crate::models::isoformat_utc(next);
    Ok(fleet)
}

/// Whether a stored schedule already runs what the declaration says.
fn matches(stored: &Schedule, wanted: &Schedule) -> bool {
    stored.cron == wanted.cron
        && stored.tz == wanted.tz
        && stored.command == wanted.command
        && stored.pinned_host == wanted.pinned_host
        && stored.secret_env.len() == wanted.secret_env.len()
        && stored.secret_env.iter().all(|(variable, reference)| {
            wanted
                .secret_env
                .get(variable)
                .is_some_and(|other| other.role == reference.role && other.field == reference.field)
        })
}

/// Make the fleet hold exactly the declared schedules of `name`: keep each
/// one that already matches, replace each one that changed, create each
/// missing one, delete each one no longer declared. Answers one report row
/// per schedule touched or kept.
pub(crate) async fn activate(name: &str, declared: &WebApiProduct) -> Result<Value, CmdError> {
    let store = JobStorage::new().await?;
    let mut stored = owned(&store, name).await?;
    let mut rows = Vec::new();
    for (schedule_name, schedule) in declared.schedules() {
        let wanted = rendered(name, declared, schedule_name, schedule).await?;
        let prefix = schedule_prefix(name, schedule_name);
        let mine: Vec<Schedule> = stored
            .iter()
            .filter(|fleet| fleet.schedule_id.starts_with(&prefix))
            .cloned()
            .collect();
        stored.retain(|fleet| !fleet.schedule_id.starts_with(&prefix));
        if let [only] = mine.as_slice() {
            if matches(only, &wanted) {
                rows.push(json!({"schedule": schedule_name, "id": only.schedule_id, "change": "unchanged"}));
                continue;
            }
        }
        for old in &mine {
            schedules::delete_schedule(&store, &old.schedule_id).await?;
        }
        schedules::write_schedule(&store, &wanted).await?;
        let change = if mine.is_empty() {
            "created"
        } else {
            "replaced"
        };
        rows.push(json!({"schedule": schedule_name, "id": wanted.schedule_id, "change": change}));
    }
    for orphan in stored {
        schedules::delete_schedule(&store, &orphan.schedule_id).await?;
        rows.push(json!({"id": orphan.schedule_id, "change": "deleted"}));
    }
    Ok(Value::Array(rows))
}

/// Delete every fleet schedule `name` owns, for `stado web remove`.
pub(crate) async fn withdraw(name: &str) -> Result<Value, CmdError> {
    let store = JobStorage::new().await?;
    let mut rows = Vec::new();
    for fleet in owned(&store, name).await? {
        schedules::delete_schedule(&store, &fleet.schedule_id).await?;
        rows.push(json!({"id": fleet.schedule_id, "change": "deleted"}));
    }
    Ok(Value::Array(rows))
}
