//! Which GitHub identity Stado acts as, and how that credential is found.
//!
//! The identity used to be named by id. `GITHUB_CREDENTIAL_ITEM` was the
//! literal `"GITHUB_TOKEN"` compiled into the runner lifecycle, so replacing
//! the credential meant editing Rust and shipping a release. On 2026-09-07
//! that cost the fleet a day: `GET /orgs/wisent-ai/actions/runner-groups`
//! answered HTTP 403 "You must be an org admin or have the runners and runner
//! groups fine-grained permission" for the identity that item holds — an OAuth
//! token carrying `read:org` where the endpoint answers only `admin:org` — and
//! there was nowhere to say "use the other one" without a code change.
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

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use check::report;

/// The declaration, and the path every refusal here names.
pub const DECLARATION_PATH: &str = "stado-rs/data/github-identity.json";
const DECLARATION: &str = include_str!("../../data/github-identity.json");
const SCHEMA: &str = "stado.github-identity.v1";
/// Skarbiec's own operator route for its route table. It used to read
/// `/v1/operator/route/resolve`, an endpoint Skarbiec has never served: the
/// resolution answered HTTP 404 and the report blamed the vault for a question
/// this caller invented. `POST /v1/operator/routes/list` is the published one,
/// and it answers `{consumer, routes:[{resource,item,item_present,field,
/// field_present}]}` — the shape read below.
const RESOLVE_ENDPOINT: &str = "/v1/operator/routes/list";
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

fn exact(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "{DECLARATION_PATH} {field} must be one exact non-empty token"
        ));
    }
    Ok(())
}

fn parse_declaration() -> Result<GithubIdentity, String> {
    let identity: GithubIdentity = serde_json::from_str(DECLARATION)
        .map_err(|error| format!("{DECLARATION_PATH} is invalid: {error}"))?;
    if identity.schema != SCHEMA {
        return Err(format!(
            "{DECLARATION_PATH} schema is {:?}, expected {SCHEMA:?}",
            identity.schema
        ));
    }
    exact(&identity.credential_route, "credential_route")?;
    exact(&identity.required_permission, "required_permission")?;
    exact(&identity.reality_check, "reality_check")?;
    // Skarbiec resolves `provider:`, `agent:` and `login:` from what a vault
    // item declares about itself, and `<item>#<field>` is an exact coordinate
    // named rather than declared. A fleet GitHub identity is neither: it is a
    // resource no item can declare, so it belongs in Skarbiec's hand-declared
    // route table and has to be shaped like one. Accepting a coordinate here
    // would put the item id back in Stado's tree under a different name.
    if identity.credential_route.contains('#')
        || SELF_DECLARING_PREFIXES
            .iter()
            .any(|prefix| identity.credential_route.starts_with(prefix))
        || !identity.credential_route.contains(':')
    {
        return Err(format!(
            "{DECLARATION_PATH} credential_route {:?} is not a declarable route: name a \
             <product>:<role> resource Skarbiec answers from its route table, such as \
             \"github:org-runner-admin\", not an item coordinate and not a prefix an item \
             declares for itself",
            identity.credential_route
        ));
    }
    if !identity.reality_check.starts_with('/') {
        return Err(format!(
            "{DECLARATION_PATH} reality_check must be an api.github.com path beginning with \"/\""
        ));
    }
    if identity.reality_check.contains("{repository}") {
        let repository = identity
            .reality_check_repository
            .as_deref()
            .unwrap_or_default();
        exact(repository, "reality_check_repository")?;
        if repository.contains('/') {
            return Err(format!(
                "{DECLARATION_PATH} reality_check_repository {repository:?} must be one \
                 repository name inside the organization"
            ));
        }
    }
    Ok(identity)
}

static IDENTITY: LazyLock<Result<GithubIdentity, String>> = LazyLock::new(parse_declaration);

/// The declaration, or the one sentence that says why it cannot be read.
pub fn declared() -> Result<&'static GithubIdentity, String> {
    IDENTITY.as_ref().map_err(String::clone)
}

fn unanswered(route: &str, detail: &str) -> String {
    format!(
        "Skarbiec answers no credential for the declared GitHub route {route:?}: {detail}. Stado \
         reads its GitHub identity through that route, declared in {DECLARATION_PATH}; declare it \
         with `skarbiec routes add --resource {route} --item <item> --field <field> --reason \
         <text>`"
    )
}

/// Which vault coordinate the declared route reaches, asked of Skarbiec.
pub async fn resolve() -> Result<ResolvedCredential, String> {
    let identity = declared()?;
    let route = identity.credential_route.as_str();
    let credentials = crate::credential_store::admin_credentials().map_err(|error| {
        format!("Stado cannot reach Skarbiec to resolve the GitHub route {route:?}: {error}")
    })?;
    let endpoint = format!(
        "{}{RESOLVE_ENDPOINT}",
        credentials.url.trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(&endpoint)
        .json(&json!({}))
        .send()
        .await
        .map_err(|error| {
            format!("Skarbiec did not answer {endpoint} for the GitHub route {route:?}: {error}")
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        format!("Skarbiec answered {endpoint} for {route:?} unreadably: {error}")
    })?;
    if !status.is_success() {
        return Err(unanswered(
            route,
            &format!(
                "Skarbiec answered HTTP {} — {}",
                status.as_u16(),
                body.trim()
            ),
        ));
    }
    let document: Value = serde_json::from_str(&body)
        .map_err(|error| format!("Skarbiec route report for {route:?} is invalid: {error}"))?;
    let row = document
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("resource").and_then(Value::as_str) == Some(route))
        })
        .ok_or_else(|| unanswered(route, "its route report names no such resource"))?;
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
            .ok_or_else(|| unanswered(route, &format!("its route report has no valid {key}")))
    };
    Ok(ResolvedCredential {
        route: route.to_string(),
        item: text("item")?,
        field: text("field")?,
        // Skarbiec's route report names the coordinate, not the reader: its
        // rows carry `resource`, `item`, `item_present`, `field` and
        // `field_present`. Who declared that Stado reads its GitHub identity
        // through this route is this file, so the declaration says so instead
        // of a field the vault was expected to invent.
        declared_by: DECLARATION_PATH.to_string(),
    })
}

/// Exactly the one field a resolved route names, through Stado's ordinary
/// credential read. Separate from [`resolve`] so a caller that already asked
/// which coordinate answered does not ask twice.
pub async fn read(resolved: &ResolvedCredential) -> Result<String, String> {
    let credentials = crate::credential_store::admin_credentials().map_err(|error| {
        format!(
            "Stado cannot reach Skarbiec to read {}.{}: {error}",
            resolved.item, resolved.field
        )
    })?;
    let client = crate::skarbiec::Client::direct(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(|error| error.to_string())?;
    client
        .read_string(&resolved.item, &resolved.field)
        .await
        .map_err(|error| error.to_string())?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "the credential the GitHub route {:?} names, {}.{}, is empty; that route is \
                 declared in {DECLARATION_PATH}",
                resolved.route, resolved.item, resolved.field
            )
        })
}

/// The credential the declaration names: resolve the route, then read it.
pub async fn credential() -> Result<String, String> {
    read(&resolve().await?).await
}
