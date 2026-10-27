//! `stado database cost --provider supabase`: what every Supabase organization
//! the management API token sees is billed for, project by project, read from
//! the API instead of the dashboard.
//!
//! For each project: its compute and add-ons with the price Supabase states
//! for each (hourly or monthly), what they come to over the current calendar
//! month, the gateway traffic of the last seven days (REST, Auth, Realtime,
//! Storage) and its total, and the declared database whose item names the
//! project. Connections made straight to Postgres are not counted by the
//! gateway, so a total of nothing is a question to ask its consumers, not
//! proof nobody uses it. Compute is billed only while a project runs, so a
//! paused project's compute is not counted.
//!
//! For each organization: its plan, the compute credit the plan includes
//! (`supabase-pricing.json`), and the month's usage after that credit. The
//! plan's own fee and the credit balance are not exposed by the management
//! API; the report names the billing page they are read on instead of
//! guessing them. An add-on whose price states another interval is listed as
//! `unpriced`, never counted as nothing.

use chrono::Datelike;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::create::{organization_of, price, RUNNING};
use super::{call, text, token};

/// The services the gateway's usage report counts.
const TRAFFIC: &[&str] = &[
    "total_rest_requests",
    "total_auth_requests",
    "total_realtime_requests",
    "total_storage_requests",
];
const COMPUTE: &str = "compute_instance";
const HOURLY: &str = "hourly";
const MONTHLY: &str = "monthly";

/// What one organization's projects come to.
#[derive(Default)]
struct Totals {
    compute_hourly: f64,
    other_hourly: f64,
    monthly: f64,
}

fn refused(what: String) -> CmdError {
    CmdError::click(what).stating(FailureCode::InfraDown)
}

/// The declared databases by the Supabase project their item names, and the
/// items that could not be read, each with its cause.
async fn declared_projects() -> Result<(BTreeMap<String, String>, Vec<String>), CmdError> {
    let mut by_reference = BTreeMap::new();
    let mut unread = Vec::new();
    for (name, declaration) in super::super::declared_databases()? {
        match crate::credential_store::read_declared_string(declaration.item(), "project_ref").await
        {
            Ok(Some(reference)) => {
                by_reference.insert(reference, name.clone());
            }
            Ok(None) => {}
            Err(error) => unread.push(format!("{}: {error}", declaration.item())),
        }
    }
    Ok((by_reference, unread))
}

/// The project's selected add-ons, compute among them.
async fn addons(reference: &str, token: &str) -> Result<Vec<Value>, CmdError> {
    let path = format!("/projects/{reference}/billing/addons");
    let answer = call(reqwest::Method::GET, &path, token, None).await?;
    answer["selected_addons"]
        .as_array()
        .cloned()
        .ok_or_else(|| {
            refused(format!(
                "Supabase GET {path} answered no selected_addons list: {answer}"
            ))
        })
}

/// The last seven days of gateway requests, summed per service, and their
/// total.
async fn traffic(reference: &str, token: &str) -> Result<(Map<String, Value>, u64), CmdError> {
    let path = format!("/projects/{reference}/analytics/endpoints/usage.api-counts?interval=7day");
    let answer = call(reqwest::Method::GET, &path, token, None).await?;
    let days = answer["result"].as_array().ok_or_else(|| {
        refused(format!(
            "Supabase GET {path} answered no result list: {answer}"
        ))
    })?;
    let mut sums = Map::new();
    let mut all = Vec::new();
    for service in TRAFFIC {
        let mut each = Vec::new();
        for day in days {
            each.push(day[*service].as_u64().ok_or_else(|| {
                refused(format!(
                    "Supabase GET {path} answered a day without a count of {service}: {day}"
                ))
            })?);
        }
        let sum: u64 = each.iter().sum();
        all.push(sum);
        sums.insert((*service).to_string(), json!(sum));
    }
    Ok((sums, all.iter().sum()))
}

/// Each add-on with its stated price, added to `totals`; compute counts only
/// while the project runs. Returns the priced lines, the unpriced ones, and
/// the project's hourly and fixed monthly sums.
fn price_addons(
    addons: &[Value],
    running: bool,
    totals: &mut Totals,
) -> (Vec<Value>, Vec<Value>, f64, f64) {
    let mut lines = Vec::new();
    let mut unpriced = Vec::new();
    let (mut hourly, mut monthly) = (Vec::new(), Vec::new());
    for addon in addons {
        let kind = addon["type"].as_str();
        let price = &addon["variant"]["price"];
        let line = json!({
            "type": addon["type"],
            "variant": addon["variant"]["id"],
            "name": addon["variant"]["name"],
            "amount_usd": price["amount"],
            "interval": price["interval"],
        });
        let (Some(amount), Some(interval)) = (price["amount"].as_f64(), price["interval"].as_str())
        else {
            unpriced.push(line);
            continue;
        };
        if interval == HOURLY {
            if kind == Some(COMPUTE) {
                if running {
                    totals.compute_hourly += amount;
                    hourly.push(amount);
                }
            } else {
                totals.other_hourly += amount;
                hourly.push(amount);
            }
            lines.push(line);
        } else if interval == MONTHLY {
            totals.monthly += amount;
            monthly.push(amount);
            lines.push(line);
        } else {
            unpriced.push(line);
        }
    }
    (lines, unpriced, hourly.iter().sum(), monthly.iter().sum())
}

pub(in crate::cli::database) async fn cost(json_output: bool) -> Result<(), CmdError> {
    let token = token().await?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None).await?;
    let projects = listed.as_array().ok_or_else(|| {
        refused(format!(
            "Supabase GET /projects answered something that is not a list: {listed}"
        ))
    })?;
    let (declared, unread) = declared_projects().await?;
    let month_hours = chrono::Duration::days(i64::from(
        chrono::Utc::now().date_naive().num_days_in_month(),
    ))
    .num_hours();
    let hours = month_hours as f64;
    let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut failures: Vec<String> = unread
        .iter()
        .map(|item| format!("declaration {item}"))
        .collect();
    for project in projects {
        let reference = project["ref"]
            .as_str()
            .filter(|reference| !reference.is_empty())
            .ok_or_else(|| {
                refused(format!(
                    "Supabase listed a project without its ref: {project}"
                ))
            })?;
        let organization = organization_of(project).ok_or_else(|| {
            refused(format!(
                "Supabase listed project {reference} without its organization: {project}"
            ))
        })?;
        let running = project["status"] == RUNNING;
        let mut row = json!({
            "project": project["name"],
            "ref": reference,
            "organization": organization,
            "region": project["region"],
            "status": project["status"],
            "declared_database": declared.get(reference),
        });
        let organization_totals = totals.entry(organization).or_default();
        match addons(reference, &token).await {
            Ok(addons) => {
                let (lines, unpriced, hourly, monthly) =
                    price_addons(&addons, running, organization_totals);
                row["addons"] = json!(lines);
                row["unpriced"] = json!(unpriced);
                row["hourly_usd"] = json!(hourly);
                row["fixed_monthly_usd"] = json!(monthly);
                row["month_usd"] = json!(hourly * hours + monthly);
            }
            Err(error) => {
                failures.push(format!("{reference} add-ons: {error}"));
                row["addons_refused"] = json!(error.to_string());
            }
        }
        match traffic(reference, &token).await {
            Ok((sums, total)) => {
                row["requests_7d"] = Value::Object(sums);
                row["requests_7d_total"] = json!(total);
            }
            Err(error) => {
                failures.push(format!("{reference} traffic: {error}"));
                row["traffic_refused"] = json!(error.to_string());
            }
        }
        rows.push(row);
    }
    let mut organizations = Vec::new();
    for (slug, sums) in &totals {
        let path = format!("/organizations/{slug}");
        let answer = call(reqwest::Method::GET, &path, &token, None).await?;
        let plan = answer["plan"]
            .as_str()
            .ok_or_else(|| refused(format!("Supabase GET {path} names no plan: {answer}")))?;
        let credit = price(if plan == "free" {
            "free_plan_compute_usd"
        } else {
            "paid_plan_compute_credit_usd"
        })? as f64;
        let compute = sums.compute_hourly * hours;
        let usage = compute - credit.min(compute) + sums.other_hourly * hours + sums.monthly;
        organizations.push(json!({
            "organization": slug,
            "plan": plan,
            "month_hours": month_hours,
            "compute_month_usd": compute,
            "compute_credit_usd": credit,
            "other_hourly_month_usd": sums.other_hourly * hours,
            "fixed_monthly_usd": sums.monthly,
            "usage_month_usd": usage,
            "plan_fee_and_credit_balance": format!(
                "not exposed by the Supabase management API; read them at {}",
                text("billing_page")?.replace("{slug}", slug)
            ),
        }));
    }
    let report = json!({ "projects": rows, "organizations": organizations, "pricing_source": text("source")? });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for line in report["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(report["organizations"].as_array().into_iter().flatten())
        {
            crate::cli::print_answer(line, false)?;
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(refused(format!(
        "{} reads failed, so the bill above is incomplete: {}",
        failures.len(),
        failures.join("; ")
    )))
}
