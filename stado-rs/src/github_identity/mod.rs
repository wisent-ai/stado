//! Which GitHub identity Stado acts as, and how that credential is found.
//!
//! The identity used to be named by id. `GITHUB_CREDENTIAL_ITEM` was the
//! literal `"GITHUB_TOKEN"` compiled into the runner lifecycle, so replacing
//! the credential meant editing Rust and shipping a release. That costs a
//! day when the organization's runner-groups endpoint answers HTTP 403 "You
//! must be an org admin or have the runners and runner groups fine-grained
//! permission" for the identity that item holds — an OAuth token carrying
//! `read:org` where the endpoint answers only `admin:org` — and there is
//! nowhere to say "use the other one" without a code change.
//!
//! The declaration names a Skarbiec route instead of an item. Skarbiec answers
//! which item and field a route reaches, reading what the vault and its route
//! table declare at the moment it is asked, so a credential that is renamed,
//! replaced, or moved to another field is still found. A route nothing answers
//! is a refusal that names the route and the command that declares it, rather
//! than a 403 that names nothing.
//!
//! Two calls, deliberately separate. Resolution asks Skarbiec's operator route
//! which coordinate a name reaches and carries no value, which is why a report
//! may print it. The read is Stado's ordinary one-field credential read of
//! exactly that coordinate. Nothing here widens what Stado can reach: the
//! consumer grant still decides, and the value never enters argv, a log, or a
//! report.

mod check;
mod declaration;

use declaration::parse_declaration;

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deploy::DeployError;
use crate::primitives::failure::FailureCode;

pub use check::report;

/// The declaration, and the path every refusal here names.
pub const DECLARATION_PATH: &str = "stado-rs/data/fleet/github-identity.json";
const DECLARATION: &str = include_str!("../../data/fleet/github-identity.json");
const SCHEMA: &str = "stado.github-identity.v1";
/// Skarbiec's declared route capability answers the current vault coordinates,
/// rather than a separately maintained per-caller route listing.
const RESOLVE_ENDPOINT: &str = "/v1/operator/route/resolve";
const SELF_DECLARING_PREFIXES: &[&str] = &["provider:", "agent:", "login:"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubIdentity {
    pub schema: String,
    /// The Skarbiec route resource that names this fleet's GitHub credential.
    pub credential_route: String,
    /// What GitHub must allow that identity to do, in GitHub's own vocabulary.
    pub required_permission: String,
    /// The endpoint that confronts the declaration with GitHub. `{organization}`
    /// and `{repository}` are substituted; a check that names a repository
    /// needs `reality_check_repository` to say which one.
    pub reality_check: String,
    /// The repository the check reads, when the check is a repository one.
    ///
    /// The fleet registers repository-scoped runners, so the door it needs is
    /// a repository's runner list, not the organization's. Naming the
    /// repository here keeps the check on that door instead of demanding an
    /// organization-administrator credential nobody asked to exist.
    #[serde(default)]
    pub reality_check_repository: Option<String>,
}

/// One resolved coordinate. The value is absent on purpose: this is the answer
/// a report is allowed to print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedCredential {
    pub route: String,
    pub item: String,
    pub field: String,
    pub declared_by: String,
}

static IDENTITY: LazyLock<Result<GithubIdentity, String>> = LazyLock::new(parse_declaration);

/// The declaration, or the one sentence that says why it cannot be read.
pub fn declared() -> Result<&'static GithubIdentity, String> {
    IDENTITY.as_ref().map_err(String::clone)
}

fn unanswered(route: &str, detail: &str) -> DeployError {
    DeployError(format!(
        "Skarbiec answers no credential for the declared GitHub route {route:?}: {detail}. Stado \
         reads its GitHub identity through that route, declared in {DECLARATION_PATH}; declare it \
         with `skarbiec route declare --resource {route} --item <item> --field <field> --reason \
         <text>`"
    ))
    .stating(FailureCode::NotFound)
}

/// `message` with the class `cause` states through its own conversion.
fn carrying(message: String, cause: impl Into<crate::cli::CmdError>) -> DeployError {
    DeployError {
        message,
        failure: cause.into().failure,
    }
}

/// Which vault coordinate the declared route reaches, asked of Skarbiec.
pub async fn resolve() -> Result<ResolvedCredential, DeployError> {
    let identity = declared().map_err(|error| DeployError(error).stating(FailureCode::Config))?;
    let route = identity.credential_route.as_str();
    let credentials = crate::credential_store::admin_credentials().map_err(|error| {
        carrying(
            format!("Stado cannot reach Skarbiec to resolve the GitHub route {route:?}: {error}"),
            error,
        )
    })?;
    let endpoint = format!(
        "{}{RESOLVE_ENDPOINT}",
        credentials.url.trim_end_matches('/')
    );
    let response = crate::wait::request(
        reqwest::Client::new()
            .post(&endpoint)
            .json(&json!({"names": [route]})),
    )
    .await
    .map_err(|error| {
        carrying(
            format!("Skarbiec did not answer {endpoint} for the GitHub route {route:?}: {error}"),
            error,
        )
    })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        carrying(
            format!("Skarbiec answered {endpoint} for {route:?} unreadably: {error}"),
            error,
        )
    })?;
    if !status.is_success() {
        let detail = format!(
            "Skarbiec at {endpoint} answered HTTP {} — {}",
            status.as_u16(),
            body.trim()
        );
        return Err(DeployError(unanswered(route, &detail).message)
            .stating(FailureCode::from_upstream_status(status.as_u16())));
    }
    let damaged = |detail: String| DeployError(detail).stating(FailureCode::InfraDown);
    let document: Value = serde_json::from_str(&body).map_err(|error| {
        damaged(format!(
            "Skarbiec route report from {endpoint} for {route:?} is invalid: {error}"
        ))
    })?;
    let rows = document
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            damaged(format!(
                "Skarbiec at {endpoint} returned no routes array for {route:?}"
            ))
        })?;
    let row = rows
        .iter()
        .find(|row| row.get("resource").and_then(Value::as_str) == Some(route))
        .ok_or_else(|| {
            let table = document
                .get("table")
                .and_then(Value::as_str)
                .unwrap_or("not reported");
            unanswered(
                route,
                &format!(
                    "the response from {endpoint} names no such resource; route table: {table}"
                ),
            )
        })?;
    if row.get("item_present") != Some(&Value::Bool(true))
        || row.get("field_present") != Some(&Value::Bool(true))
    {
        let problem = row
            .get("problem")
            .and_then(Value::as_str)
            .unwrap_or("the route resolves to no readable field");
        return Err(unanswered(route, problem));
    }
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .map(str::to_string)
            .ok_or_else(|| {
                damaged(format!(
                    "Skarbiec's route report for {route:?} has no valid {key}"
                ))
            })
    };
    Ok(ResolvedCredential {
        route: route.to_string(),
        item: text("item")?,
        field: text("field")?,
        // Skarbiec's route report names the coordinate, not the reader: who
        // declared that Stado reads its GitHub identity through this route is
        // this file, so the declaration says so.
        declared_by: DECLARATION_PATH.to_string(),
    })
}

/// Exactly the one field a resolved route names, through Stado's ordinary
/// credential read. Separate from [`resolve`] so a caller that already asked
/// which coordinate answered does not ask twice.
pub async fn read(resolved: &ResolvedCredential) -> Result<String, DeployError> {
    let coordinate = format!("{}.{}", resolved.item, resolved.field);
    let credentials = crate::credential_store::admin_credentials().map_err(|error| {
        carrying(
            format!("Stado cannot reach Skarbiec to read {coordinate}: {error}"),
            error,
        )
    })?;
    let client = crate::skarbiec::Client::direct(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(|error| {
        carrying(
            format!("Stado cannot open Skarbiec to read {coordinate}: {error}"),
            error,
        )
    })?;
    client
        .read_declared_string(&resolved.item, &resolved.field)
        .await
        .map_err(|error| {
            carrying(
                format!("Skarbiec did not hand over {coordinate}: {error}"),
                error,
            )
        })?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "the credential the GitHub route {:?} names, {coordinate}, is empty; that route is \
                 declared in {DECLARATION_PATH}",
                resolved.route
            ))
            .stating(FailureCode::NotFound)
        })
}

/// The credential the declaration names: resolve the route, then read it.
pub async fn credential() -> Result<String, DeployError> {
    read(&resolve().await?).await
}
