//! The Supabase section of the billing snapshot: whether the hosted project
//! behind each declared database is running, read from the management API
//! on every billing pass.
//!
//! A database whose only copy lives in a Supabase project stops answering
//! when the project is paused or removed, and the provider's notice of it
//! (a failed payment, a pending shutdown, an inactive free project) reached
//! no product. This section names every declared database whose item names
//! a project (`project_ref`), the status Supabase reports for that project,
//! its organization's plan, and the way off the provider. The section is
//! `ok` only when every such project reports [`RUNNING`]; a project that
//! reports anything else makes it `project_not_running`, a project Supabase
//! does not list for the token makes it `project_not_listed`, and an item or
//! answer that cannot be read makes it `error` with the cause, so the billing
//! health fold alerts on each from the pass it first appears in. Supabase's
//! own status word is reported as it gave it; no meaning of ours is put on
//! it beyond "not running".

use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::cli::CmdError;

use super::super::create::{organization_of, RUNNING};
use super::super::{call, token};

/// The section as the billing collector publishes it. Never an error: a
/// read that fails is the section's own status and detail.
pub(crate) async fn section() -> Value {
    match read().await {
        Ok(section) => section,
        Err(Unread { status, detail }) => json!({ "status": status, "detail": detail }),
    }
}

/// Why the section could not be read at all.
struct Unread {
    status: &'static str,
    detail: String,
}

impl Unread {
    fn from(status: &'static str, error: CmdError) -> Self {
        Self {
            status,
            detail: error.to_string(),
        }
    }
}

async fn read() -> Result<Value, Unread> {
    // A database section that does not parse is the operator's configuration
    // naming something invalid: the section says so as config_error.
    let (hosted, unread) = super::hosted_projects()
        .await
        .map_err(|error| Unread::from("config_error", error))?;
    if hosted.is_empty() && unread.is_empty() {
        return Ok(json!({
            "status": "ok",
            "detail": "no declared database names a Supabase project",
            "databases": [],
        }));
    }
    let token = token()
        .await
        .map_err(|error| Unread::from("no_credentials", error))?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None)
        .await
        .map_err(|error| Unread::from("error", error))?;
    let projects = listed.as_array().ok_or_else(|| Unread {
        status: "error",
        detail: format!("Supabase GET /projects answered something that is not a list: {listed}"),
    })?;
    let mut plans: BTreeMap<String, Result<String, String>> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut not_running = Vec::new();
    let mut not_listed = Vec::new();
    for (name, reference) in &hosted {
        let alternative = format!("stado database migrate {name} --to fleet");
        let Some(project) = projects.iter().find(|project| project["ref"] == *reference) else {
            not_listed.push(format!(
                "database {name}: Supabase lists no project {reference} for this token (removed, \
                 or held by an account the token does not reach); its consumers cannot reach it; \
                 the way off the provider: {alternative}"
            ));
            rows.push(json!({
                "database": name,
                "project_ref": reference,
                "listed": false,
                "alternative": alternative,
            }));
            continue;
        };
        let status = project["status"].as_str();
        let organization = organization_of(project);
        let plan = match &organization {
            Some(slug) => {
                if !plans.contains_key(slug) {
                    let path = format!("/organizations/{slug}");
                    let read = match call(reqwest::Method::GET, &path, &token, None).await {
                        Ok(answer) => answer["plan"]
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("Supabase GET {path} names no plan: {answer}")),
                        Err(error) => Err(error.to_string()),
                    };
                    plans.insert(slug.clone(), read);
                }
                plans.get(slug).cloned()
            }
            None => None,
        };
        let running = status == Some(RUNNING);
        if !running {
            not_running.push(format!(
                "database {name}: Supabase project {} ({reference}) reports {}, not {RUNNING}, on \
                 organization {}{}; its consumers cannot reach it until it runs again; restore it \
                 at Supabase, or the way off the provider: {alternative}",
                project["name"].as_str().unwrap_or("without a name"),
                status.unwrap_or("no status"),
                organization.as_deref().unwrap_or("not named"),
                match &plan {
                    Some(Ok(plan)) => format!(" (plan {plan})"),
                    Some(Err(cause)) => format!(" (plan unread: {cause})"),
                    None => String::new(),
                },
            ));
        }
        rows.push(json!({
            "database": name,
            "project_ref": reference,
            "project": project["name"],
            "listed": true,
            "project_status": status,
            "running": running,
            "organization": organization,
            "plan": plan.as_ref().and_then(|plan| plan.as_ref().ok()),
            "plan_unread": plan.as_ref().and_then(|plan| plan.as_ref().err()),
            "alternative": alternative,
        }));
    }
    let (status, findings) = if !not_listed.is_empty() {
        (
            "project_not_listed",
            [not_listed.clone(), not_running.clone()].concat(),
        )
    } else if !not_running.is_empty() {
        ("project_not_running", not_running.clone())
    } else if !unread.is_empty() {
        ("error", Vec::new())
    } else {
        ("ok", Vec::new())
    };
    let mut detail: Vec<String> = findings;
    if !unread.is_empty() {
        detail.push(format!(
            "items that could not be read to tell whether their database lives in Supabase: {}",
            unread.join("; ")
        ));
    }
    Ok(json!({
        "status": status,
        "detail": detail.join("; "),
        "databases": rows,
        "not_running": not_running.len() + not_listed.len(),
        "unread_items": unread,
    }))
}
