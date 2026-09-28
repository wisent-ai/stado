//! `stado database create`: a hosted database the fleet does not have yet.
//!
//! The provider is Supabase, the organization every declared database lives
//! in. Before anything is created the command reads what one more project
//! adds to the bill — the organization's plan, its running projects, and the
//! compute prices in `supabase-pricing.json` — and refuses unless
//! `--accept-monthly-usd` covers that figure. The management API does not
//! expose the credit balance; the report names the billing page it is on
//! instead of guessing it. A project that already carries the name is
//! reused, never created twice. The credential item `<name>-database` is
//! written with the project's coordinates and the generated password into the
//! fleet's owner vault — directly on the owner host, through the host channel
//! from any other — and the database is declared for its consumers as
//! `declare` does.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::owner_vault;
use super::{call, field, item_fields, pooler, provider, text, TOKEN_ITEM};

const RUNNING: &str = "ACTIVE_HEALTHY";

fn price(key: &str) -> u64 {
    provider()[key]
        .as_u64()
        .unwrap_or_else(|| panic!("supabase-pricing.json declares no {key}"))
}

fn organization_of(project: &Value) -> Option<String> {
    project
        .get("organization_slug")
        .or_else(|| project.get("organization_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// What one more running project adds to the organization's monthly bill.
pub(super) fn cost(plan: &str, running: usize) -> u64 {
    if plan == "free" {
        return price("free_plan_compute_usd");
    }
    let covered = (price("paid_plan_compute_credit_usd") / price("micro_monthly_usd")) as usize;
    if running < covered {
        0
    } else {
        price("micro_monthly_usd")
    }
}

/// The organization, region and bill of one more project beside `anchor`;
/// refuses unless `accepted` covers the added monthly compute.
async fn priced_creation(
    name: &str,
    anchor: &str,
    token: &str,
    projects: &[Value],
    accepted: Option<u64>,
) -> Result<(String, String, Value), CmdError> {
    let anchor_ref = field(&format!("{anchor}-database"), "project_ref").await?;
    let anchor_project = projects
        .iter()
        .find(|p| p["ref"] == anchor_ref.as_str())
        .ok_or_else(|| {
            CmdError::click(format!(
                "anchor project {anchor_ref} is not visible to {TOKEN_ITEM}"
            ))
            .stating(crate::primitives::failure::FailureCode::Refused)
        })?;
    let slug = organization_of(anchor_project).ok_or_else(|| {
        CmdError::click(format!("anchor project {anchor_ref} names no organization"))
            .stating(crate::primitives::failure::FailureCode::Refused)
    })?;
    let region = anchor_project["region"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let org = call(
        reqwest::Method::GET,
        &format!("/organizations/{slug}"),
        token,
        None,
    )
    .await?;
    let plan = org["plan"].as_str().unwrap_or("unknown").to_string();
    let running = projects
        .iter()
        .filter(|p| p["status"] == RUNNING && organization_of(p).as_deref() == Some(slug.as_str()))
        .count();
    let added = cost(&plan, running);
    let report = json!({
        "organization": slug,
        "plan": plan,
        "running_projects": running,
        "added_monthly_compute_usd": added,
        "pricing_source": text("source"),
        "credit_balance": format!(
            "not exposed by the Supabase management API; read it at {}",
            text("billing_page").replace("{slug}", &slug)
        ),
    });
    if accepted.is_none_or(|accepted| accepted < added) {
        // A cost the operator has not accepted is a refusal of the request,
        // not an unattributed failure: say what to accept and where the
        // credit balance can be read.
        return Err(CmdError::click(format!(
            "one more project adds ${added}/month compute to {slug} (plan {plan}, {running} running; {}); \
             {name} was not created. Report: {report}",
            text("source")
        ))
        .stating(crate::primitives::failure::FailureCode::Refused)
        .helping(format!(
            "pass --accept-monthly-usd {added} to accept it; the credit balance is at {}",
            text("billing_page").replace("{slug}", &slug)
        )));
    }
    Ok((slug, region, report))
}

pub(in crate::cli::database) async fn create(
    name: &str,
    anchor: &str,
    consumers: &[String],
    accept_monthly_usd: Option<u64>,
    json_output: bool,
) -> Result<(), CmdError> {
    if !super::super::writes::canonical_name(name) {
        return Err(CmdError::usage(
            "NAME must be lowercase letters, digits and dashes",
        ));
    }
    let token = field(TOKEN_ITEM, "value").await?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None).await?;
    let projects = listed.as_array().cloned().unwrap_or_default();
    let item = format!("{name}-database");
    let owner = owner_vault::locate().await?;

    let (project, password, report) = match projects.iter().find(|p| p["name"] == name) {
        Some(existing) => {
            // The item is rewritten whole, so the password it already holds is
            // read from the owner vault first; without it the rewrite would
            // erase the only copy.
            let password = owner
                .password(&item)
                .await
                .map_err(|error| CmdError::click(format!("{item} was not rewritten: {error}")))?;
            (existing.clone(), Some(password), json!({ "reused": true }))
        }
        None => {
            let (slug, region, report) =
                priced_creation(name, anchor, &token, &projects, accept_monthly_usd).await?;
            // The generated password exists only in this process until the
            // item holds it, so an owner vault that cannot take the write
            // must stop the creation before the project exists.
            owner
                .ready()
                .map_err(|error| CmdError::click(format!("{name} was not created: {error}")))?;
            let password = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let body = json!({
                "name": name,
                "organization_slug": slug,
                "db_pass": password,
                "region_selection": { "type": "specific", "code": region },
                "desired_instance_size": text("created_instance_size"),
            });
            let created = call(reqwest::Method::POST, "/projects", &token, Some(&body)).await?;
            (created, Some(password), report)
        }
    };

    let reference = project["ref"].as_str().unwrap_or_default().to_string();
    let pooler = pooler(&reference, &token).await;
    let fields = item_fields(name, &project, pooler.as_ref(), password.as_deref());
    let context = json!({ "engine": "postgres", "provider": "supabase", "product": name });
    owner.store(&item, "bundle", &fields, &context).await?;

    let declared = super::super::verbs::declaration(
        name,
        "postgres",
        &["read".to_string(), "write".to_string()],
        consumers,
    )?;
    let outcome = json!({
        "created": name,
        "project_ref": reference,
        "status": project["status"],
        "item": item,
        "item_vault": owner.name(),
        "pooler": pooler.is_some(),
        "password_on_item": password.is_some(),
        "cost": report,
        "declaration": declared,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "database {name}: project {reference} ({}), item {item} in {}'s owner vault; pooler {}, password {}; cost {report}",
            project["status"].as_str().unwrap_or("unknown"),
            owner.name(),
            if pooler.is_some() { "recorded" } else { "not reported yet: run create again to fill it" },
            if password.is_some() { "on the item" } else { "unknown" },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cost;

    /// A paid plan's compute credit covers the first running project only; the
    /// free plan charges nothing for compute.
    #[test]
    fn one_more_project_costs_what_the_published_prices_say() {
        assert_eq!(cost("pro", 0), 0);
        assert_eq!(cost("pro", 11), 10);
        assert_eq!(cost("free", 1), 0);
    }
}
