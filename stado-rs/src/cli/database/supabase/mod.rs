//! Supabase, the provider every declared hosted database lives in: its
//! management API, the access token in Skarbiec, and the fields a database's
//! credential item `<name>-database` carries for one project.

pub(super) mod adopt;
pub(super) mod create;
mod owner_vault;

use serde_json::{json, Value};

use crate::cli::CmdError;

const API: &str = "https://api.supabase.com/v1";
const TOKEN_ITEM: &str = "SUPABASE_ACCESS_TOKEN";
const PROVIDER: &str = include_str!("provider/supabase-pricing.json");
/// Supabase Root 2021 CA, as Supabase publishes it for verifying its
/// database and pooler certificates.
const SUPABASE_ROOT_CA: &str = include_str!("provider/supabase-root-ca.pem");

fn provider() -> Value {
    serde_json::from_str(PROVIDER).expect("supabase-pricing.json is valid JSON")
}

fn text(key: &str) -> String {
    provider()[key].as_str().unwrap_or_default().to_string()
}

/// One management API call: its HTTP status and body, whatever the status.
async fn answer(
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<(reqwest::StatusCode, String), CmdError> {
    // The management API answers 403 to a request without an agent string.
    let mut request = reqwest::Client::new()
        .request(method.clone(), format!("{API}{path}"))
        .bearer_auth(token)
        .header("User-Agent", "stado-database");
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| CmdError::click(format!("Supabase {method} {path}: {error}")))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| {
        CmdError::click(format!(
            "Supabase {method} {path}: body unreadable: {error}"
        ))
    })?;
    Ok((status, text))
}

/// One management API call that must succeed, as JSON.
async fn call(
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<Value, CmdError> {
    let (status, text) = answer(method.clone(), path, token, body).await?;
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Supabase {method} {path} answered {status}: {text}"
        )));
    }
    serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("Supabase {method} {path}: {error}")))
}

/// One string field through the configured credential store: a read needs
/// no owner vault, only a grant.
async fn field(item: &str, name: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, name)
        .await
        .map_err(|error| CmdError::click(format!("{item}.{name}: {error}")))?
        .ok_or_else(|| CmdError::click(format!("{item} has no field {name}")))
}

async fn token() -> Result<String, CmdError> {
    field(TOKEN_ITEM, "value").await
}

/// The project's primary pooler, when the project reports one yet.
async fn pooler(reference: &str, token: &str) -> Option<Value> {
    call(
        reqwest::Method::GET,
        &format!("/projects/{reference}/config/database/pooler"),
        token,
        None,
    )
    .await
    .ok()?
    .as_array()?
    .iter()
    .find(|row| row["database_type"] == "PRIMARY")
    .cloned()
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
        // The provider's root CA: Supabase signs its Postgres and pooler
        // certificates with its own root, which no public trust store holds,
        // so a consumer verifying TLS needs it beside the address.
        "ca_certificate": SUPABASE_ROOT_CA,
    });
    if let Some(pooler) = pooler {
        fields["pooler_host"] = pooler["db_host"].clone();
        fields["pooler_port"] = json!(pooler["db_port"].to_string());
        fields["db_user"] = pooler["db_user"].clone();
    }
    if let Some(password) = password {
        fields["db_password"] = json!(password);
        let secret = userinfo(password);
        fields["direct_url"] = json!(format!(
            "postgresql://postgres:{secret}@db.{reference}.supabase.co:{port}/postgres"
        ));
        if let Some(pooler) = pooler {
            fields["pooler_url"] = json!(format!(
                "postgresql://{}:{secret}@{}:{}/postgres",
                userinfo(pooler["db_user"].as_str().unwrap_or_default()),
                pooler["db_host"].as_str().unwrap_or_default(),
                pooler["db_port"]
            ));
        }
    }
    fields
}

/// A user or password as it may stand in a URL's userinfo: every byte but
/// the unreserved characters percent-encoded, so `#`, `/`, `?`, `@`, `:` or
/// a literal `%` in a password reach the server as themselves.
fn userinfo(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
