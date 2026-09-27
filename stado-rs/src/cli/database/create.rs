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
//! written with the project's coordinates and the generated password, and
//! the database is declared for its consumers as `declare` does.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::credential_store::owner;

const API: &str = "https://api.supabase.com/v1";
const TOKEN_ITEM: &str = "SUPABASE_ACCESS_TOKEN";
const RUNNING: &str = "ACTIVE_HEALTHY";
const PROVIDER: &str = include_str!("supabase-pricing.json");

fn provider() -> Value {
    serde_json::from_str(PROVIDER).expect("supabase-pricing.json is valid JSON")
}

fn price(key: &str) -> u64 {
    provider()[key]
        .as_u64()
        .unwrap_or_else(|| panic!("supabase-pricing.json declares no {key}"))
}

fn text(key: &str) -> String {
    provider()[key].as_str().unwrap_or_default().to_string()
}

async fn call(
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<Value, CmdError> {
    // The management API answers 403 to a request without an agent string.
    let mut request = reqwest::Client::new()
        .request(method.clone(), format!("{API}{path}"))
        .bearer_auth(token)
        .header("User-Agent", "stado-database-create");
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| CmdError::click(format!("Supabase {method} {path}: {error}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Supabase {method} {path} answered {status}: {body}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|error| CmdError::click(format!("Supabase {method} {path}: {error}")))
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

/// One string field through the configured credential store: a read needs
/// no owner vault, only a grant.
async fn field(item: &str, name: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, name)
        .await
        .map_err(|error| CmdError::click(format!("{item}.{name}: {error}")))?
        .ok_or_else(|| CmdError::click(format!("{item} has no field {name}")))
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
        })?;
    let slug = organization_of(anchor_project).ok_or_else(|| {
        CmdError::click(format!("anchor project {anchor_ref} names no organization"))
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
        return Err(CmdError::click(format!(
            "one more project adds ${added}/month compute to {slug} (plan {plan}, {running} running; {}); \
             pass --accept-monthly-usd {added} to create {name}. Report: {report}",
            text("source")
        )));
    }
    Ok((slug, region, report))
}

/// The credential item's fields: coordinates, the pooler when the project
/// reports one, and the password with its connection strings when known.
fn item_fields(
    name: &str,
    project: &Value,
    pooler: Option<&Value>,
    password: Option<&str>,
) -> Value {
    let reference = project["ref"].as_str().unwrap_or_default();
    let port = text("direct_port");
    let mut fields = json!({
        "project_ref": reference,
        "project_name": name,
        "region": project["region"],
        "url": format!("https://{reference}.supabase.co"),
        "db_host": format!("db.{reference}.supabase.co"),
        "db_port": port,
        "db_name": "postgres",
    });
    if let Some(pooler) = pooler {
        fields["pooler_host"] = pooler["db_host"].clone();
        fields["pooler_port"] = json!(pooler["db_port"].to_string());
        fields["db_user"] = pooler["db_user"].clone();
    }
    if let Some(password) = password {
        fields["db_password"] = json!(password);
        fields["direct_url"] = json!(format!(
            "postgresql://postgres:{password}@db.{reference}.supabase.co:{port}/postgres"
        ));
        if let Some(pooler) = pooler {
            fields["pooler_url"] = json!(format!(
                "postgresql://{}:{password}@{}:{}/postgres",
                pooler["db_user"].as_str().unwrap_or_default(),
                pooler["db_host"].as_str().unwrap_or_default(),
                pooler["db_port"]
            ));
        }
    }
    fields
}

pub(super) async fn create(
    name: &str,
    anchor: &str,
    consumers: &[String],
    accept_monthly_usd: Option<u64>,
    json_output: bool,
) -> Result<(), CmdError> {
    if !super::writes::canonical_name(name) {
        return Err(CmdError::usage(
            "NAME must be lowercase letters, digits and dashes",
        ));
    }
    let token = field(TOKEN_ITEM, "value").await?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None).await?;
    let projects = listed.as_array().cloned().unwrap_or_default();
    let item = format!("{name}-database");

    let (project, password, report) = match projects.iter().find(|p| p["name"] == name) {
        Some(existing) => (
            existing.clone(),
            field(&item, "db_password").await.ok(),
            json!({ "reused": true }),
        ),
        None => {
            let (slug, region, report) =
                priced_creation(name, anchor, &token, &projects, accept_monthly_usd).await?;
            // The generated password exists only in this process until the
            // item holds it, so a host that cannot write the item must not
            // create the project.
            owner::vault().map_err(|error| {
                CmdError::click(format!(
                    "{item} cannot be written here, so {name} was not created: {error}. \
                     Run stado database create on the vault owner host."
                ))
            })?;
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
    let pooler = call(
        reqwest::Method::GET,
        &format!("/projects/{reference}/config/database/pooler"),
        &token,
        None,
    )
    .await
    .ok()
    .and_then(|rows| {
        rows.as_array()?
            .iter()
            .find(|row| row["database_type"] == "PRIMARY")
            .cloned()
    });
    let fields = item_fields(name, &project, pooler.as_ref(), password.as_deref());
    let context = json!({ "engine": "postgres", "provider": "supabase", "product": name });
    owner::write_item(&item, "bundle", &fields, &context)
        .map_err(|error| CmdError::click(error.to_string()))?;

    let declared = super::verbs::declaration(
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
        "pooler": pooler.is_some(),
        "password_on_item": password.is_some(),
        "cost": report,
        "declaration": declared,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "database {name}: project {reference} ({}), item {item}; pooler {}, password {}; cost {report}",
            project["status"].as_str().unwrap_or("unknown"),
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
