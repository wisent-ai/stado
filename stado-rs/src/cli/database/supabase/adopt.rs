//! `stado database adopt [NAME] [--project-ref REF]`: bring a declared
//! database's credential item in line with its hosted Supabase project.
//!
//! The item `<name>-database` is rewritten from the management API — the
//! project's coordinates, its pooler, its revealed API keys (the legacy
//! `anon` and `service_role`, and every named publishable or secret key as
//! `publishable_key_<name>` / `secret_key_<name>`), and its active custom
//! hostname as `custom_url` — so a rotated key lands on the next run. Fields
//! another owner put on the item stay; the password is kept from the item or
//! taken from `--password-file`, never generated. Without NAME every declared
//! database whose item names a Supabase `project_ref` is adopted again. The
//! item is read whole from the owner vault, so the command runs on the fleet's
//! owner vault host. `--check` writes nothing and exits non-zero on drift.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::credential_store::owner;

use super::owner_vault::{self, Owner};
use super::{answer, call, item_fields, pooler, token};

/// Characters a masked key is shown with; a masked value stored as a
/// credential is worse than none.
const MASK_MARKS: [char; 2] = ['·', '…'];

/// The field name a named key is stored under: lowercase, runs of other
/// characters as one `_`, or the key id's first characters when nothing is left.
fn key_slug(key: &Value) -> String {
    let id = key["id"].as_str().unwrap_or_default();
    let name = key["name"].as_str().unwrap_or(id).to_lowercase();
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        id.chars().take(8).collect()
    } else {
        slug
    }
}

/// The project's revealed API keys as item fields.
async fn key_fields(reference: &str, token: &str) -> Result<Map<String, Value>, CmdError> {
    let path = format!("/projects/{reference}/api-keys?reveal=true");
    let keys = call(reqwest::Method::GET, &path, token, None).await?;
    let keys = keys.as_array().cloned().unwrap_or_default();
    let mut fields = Map::new();
    for key in &keys {
        let value = key["api_key"].as_str().unwrap_or_default();
        if value.contains(MASK_MARKS) {
            return Err(CmdError::refused(format!(
                "project {reference}: key {} came back masked; the access token cannot reveal it",
                key["name"]
            )));
        }
        match key["type"].as_str() {
            Some("legacy") => {
                let field = match key["id"].as_str() {
                    Some("anon") => Some("anon_key"),
                    Some("service_role") => Some("service_role_key"),
                    _ => None,
                };
                if let Some(field) = field {
                    fields.insert(field.to_string(), json!(value));
                }
            }
            Some(kind @ ("publishable" | "secret")) => {
                fields.insert(format!("{kind}_key_{}", key_slug(key)), json!(value));
            }
            _ => {}
        }
    }
    for legacy in ["anon_key", "service_role_key"] {
        if !fields.contains_key(legacy) {
            return Err(
                CmdError::click(format!("project {reference} reports no legacy {legacy}"))
                    .stating(crate::primitives::failure::FailureCode::NotFound),
            );
        }
    }
    Ok(fields)
}

/// The address a project's active custom hostname serves, when it has one.
/// The route answers 400 on a project without the add-on and 404 on one
/// without a hostname; both mean there is none.
async fn custom_url(reference: &str, token: &str) -> Result<Option<String>, CmdError> {
    let path = format!("/projects/{reference}/custom-hostname");
    let (status, body) = answer(reqwest::Method::GET, &path, token, None).await?;
    if matches!(status.as_u16(), 400 | 404) {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(
            CmdError::click(format!("Supabase GET {path} answered {status}: {body}")).stating(
                crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16()),
            ),
        );
    }
    let hostname: Value = serde_json::from_str(&body).map_err(|error| {
        CmdError::click(format!("Supabase GET {path}: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let active = hostname["data"]["result"]["ssl"]["status"] == "active";
    Ok(hostname["custom_hostname"]
        .as_str()
        .filter(|name| active && !name.is_empty())
        .map(|name| format!("https://{name}")))
}

/// Every field the item should carry for this project.
async fn project_fields(
    project: &Value,
    token: &str,
    password: Option<&str>,
) -> Result<Map<String, Value>, CmdError> {
    let reference = project["ref"].as_str().unwrap_or_default();
    let name = project["name"].as_str().unwrap_or(reference);
    let pooler = pooler(reference, token).await;
    let mut fields = item_fields(name, project, pooler.as_ref(), password)
        .as_object()
        .cloned()
        .unwrap_or_default();
    fields.extend(key_fields(reference, token).await?);
    if let Some(url) = custom_url(reference, token).await? {
        fields.insert("custom_url".into(), json!(url));
    }
    Ok(fields)
}

/// Adopt one database; its report row, and whether its item drifted.
/// `project_ref` and `password` come from the command line, when given; the
/// item's own `project_ref` and `db_password` stand otherwise.
#[allow(clippy::too_many_arguments)]
async fn adopt_one(
    owner: &Owner,
    name: &str,
    item: &str,
    project_ref: Option<&str>,
    password: Option<String>,
    projects: &[Value],
    token: &str,
    check: bool,
) -> Result<(Value, bool), CmdError> {
    let document = owner::read_document(item)
        .map_err(|error| {
            CmdError::click(format!("{item} could not be read: {error}"))
                .stating(error.failure_code())
        })?
        .unwrap_or_else(|| json!({}));
    let existing = document["fields"].as_object().cloned().unwrap_or_default();
    let mut context = document["context"].as_object().cloned().unwrap_or_default();
    let supabase_item = context
        .get("provider")
        .is_none_or(|provider| provider == "supabase");
    let recorded = existing
        .get("project_ref")
        .and_then(Value::as_str)
        .filter(|_| supabase_item);
    let Some(reference) = project_ref.or(recorded).map(str::to_string) else {
        let row = json!({"database": name, "item": item, "status": "not a Supabase project item"});
        return Ok((row, false));
    };
    let project = projects
        .iter()
        .find(|project| project["ref"] == reference.as_str())
        .ok_or_else(|| {
            CmdError::click(format!(
                "project {reference} is not in the organization the access token can see"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let password = password.or_else(|| {
        existing
            .get("db_password")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let mut merged = existing.clone();
    merged.extend(project_fields(project, token, password.as_deref()).await?);
    let drifted = merged != existing;
    let status = match (drifted, check) {
        (false, _) => "up to date",
        (true, true) => "would write",
        (true, false) => {
            context.entry("engine").or_insert(json!("postgres"));
            context.insert("provider".into(), json!("supabase"));
            context.entry("product").or_insert(json!(name));
            let fields = Value::Object(merged.clone());
            owner
                .store(item, "bundle", &fields, &Value::Object(context))
                .await?;
            "written"
        }
    };
    let row = json!({
        "database": name,
        "item": item,
        "project_ref": reference,
        "status": status,
        "fields": merged.len(),
        "password_on_item": password.is_some(),
    });
    Ok((row, drifted))
}

pub(in crate::cli::database) async fn adopt(
    name: Option<&str>,
    project_ref: Option<&str>,
    password_file: Option<&str>,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if name.is_none() && (project_ref.is_some() || password_file.is_some()) {
        return Err(CmdError::usage(
            "--project-ref and --password-file adopt one database: name it",
        ));
    }
    let owner = owner_vault::locate().await?;
    if let Owner::Host(host) = &owner {
        return Err(CmdError::refused(format!(
            "the owner vault is on {host}; adopt reads each item whole, so run it there"
        )));
    }
    let password = match password_file {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|error| CmdError::click(format!("{path}: {error}")))?
                .trim()
                .to_string(),
        ),
        None => None,
    };
    let declared = super::super::declared_databases()?;
    let mut targets = Vec::new();
    for (declared_name, database) in declared {
        if name.is_none_or(|name| name == declared_name.as_str()) {
            targets.push((declared_name.to_string(), database.item().to_string()));
        }
    }
    if let Some(name) = name.filter(|_| targets.is_empty()) {
        return Err(CmdError::click(format!(
            "{name} is not declared; declare it first with stado database declare {name}"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let token = token().await?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None).await?;
    let projects = listed.as_array().cloned().unwrap_or_default();
    let mut rows = Vec::new();
    let mut drift = 0;
    for (database, item) in &targets {
        let (row, drifted) = adopt_one(
            &owner,
            database,
            item,
            project_ref,
            password.clone(),
            &projects,
            &token,
            check,
        )
        .await?;
        drift += usize::from(drifted);
        rows.push(row);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!(
                "{}: {} ({} fields)",
                row["item"].as_str().unwrap_or_default(),
                row["status"].as_str().unwrap_or_default(),
                row["fields"]
            );
        }
    }
    if check && drift > 0 {
        return Err(CmdError::refused(format!(
            "{drift} item(s) differ from their Supabase projects"
        )));
    }
    Ok(())
}
