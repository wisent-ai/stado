//! What the management API says a hosted Supabase project's credential item
//! should carry: its coordinates, pooler, revealed keys and custom hostname,
//! and a newly rotated database password.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;

use super::super::{answer, call, item_fields, pooler};

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
pub(super) async fn project_fields(
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

/// Set a new database password on the project and answer it. The password
/// exists only in this process until the item holds it, so the caller
/// stores it next; a store that fails after this leaves the project with a
/// password no item holds, and the refusal says to rotate again.
pub(super) async fn rotated_password(reference: &str, token: &str) -> Result<String, CmdError> {
    let password = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let path = format!("/projects/{reference}/database/password");
    call(
        reqwest::Method::PATCH,
        &path,
        token,
        Some(&json!({ "password": password })),
    )
    .await
    .map_err(|error| {
        let mut wrapped = CmdError::click(format!(
            "the database password of {reference} was not rotated: {error}"
        ));
        wrapped.failure = error.failure;
        wrapped
    })?;
    Ok(password)
}
