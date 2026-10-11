//! The model-review bearer a repository's CI presents to Brama, and the route
//! and grant that make it answerable.

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::brama::{
    brama_identity_host, brama_skarbiec_context,
};
use crate::deploy::host_precheck_runner::accounts::github::{
    github_credential, repository_name, set_repository_secret, GITHUB_ORGANIZATION,
};
use crate::deploy::host_precheck_runner::declaration::runner_target;
use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{host_channel, DeployError};

mod gateway;
use gateway::{
    reconcile_brama_introspection_grant, reconcile_model_review_route, verify_model_review_bearer,
};

pub const MODEL_REVIEW_SECRET: &str = "BRAMA_MODEL_ROUTER_TOKEN";
const MODEL_REVIEW_ALIAS: &str = "wisent-backend/evaluation";
/// The logical service the model-review calls go to. Its address is the one
/// Stado's service directory published for this machine, never a hostname
/// written here.
const BRAMA_SERVICE: &str = "brama";
const MODEL_REVIEW_PRIMARY_ROUTE: &str = "best";
/// The role whose item holds Brama Desktop's model-router bearer: the host's
/// Skarbiec reads the one live item tagged `stado:role:<role>`, so no item id
/// is written here.
const BRAMA_DESKTOP_MODEL_ROUTER_ROLE: &str = "role:brama-desktop-model-router";
const MODEL_REVIEW_AGENT_AUDIENCE: &str = "weles";
const BRAMA_INTROSPECTION_CONSUMER: &str = "brama-token-introspector";
const BRAMA_INTROSPECTION_CAPABILITY: &str = "introspect:tokens";
const BRAMA_INTROSPECTION_TOKEN_FILE: &str = "brama-token-introspector-skarbiec-token";

/// The body `PUT /v1/admin/routes` receives for the model-review alias: the
/// alias, and the one route it resolves to.
///
/// The third key Brama's endpoint accepts — the ordered alternates tried after
/// the primary — is absent rather than sent empty. Brama's `AdminRouteUpdate`
/// declares that field `#[serde(default)]` with type `Vec<String>`, in
/// `brama/src/core/server.rs` (the struct spanning lines 3448 through 3455), so
/// an absent key deserializes to an empty vector: the same value an explicit
/// empty list produced, and the same number of iterations in
/// `update_admin_route`, which is none. `deny_unknown_fields` on that struct
/// does not apply here, because this omits a declared field rather than adding
/// an undeclared one.
///
/// This is the one place in the runner lifecycle that leans on a server-side
/// default, so the shape is pinned by
/// `tests/runner/main.rs::the_model_review_route_request_carries_the_alias_and_its_one_route`
/// rather than left to be rediscovered when Brama's declaration next moves.
pub fn model_review_route_request() -> Value {
    json!({
        "alias": MODEL_REVIEW_ALIAS,
        "primary": MODEL_REVIEW_PRIMARY_ROUTE,
    })
}

fn model_review_client_id(repository: &str) -> String {
    let mut client_id = String::from("github-");
    for byte in repository.bytes() {
        let normalized = if byte.is_ascii_alphanumeric() {
            byte.to_ascii_lowercase() as char
        } else {
            '-'
        };
        if normalized != '-' || !client_id.ends_with('-') {
            client_id.push(normalized);
        }
    }
    client_id.push_str("-model-review");
    client_id
}
pub async fn reconcile_model_review_secret(
    target_name: &str,
    repository: &str,
) -> Result<Value, DeployError> {
    let repository = repository_name(repository)?;
    let target = runner_target(target_name).await?;
    // Beside Brama, not beside the runner. The Brama-owned Skarbiec vault, its
    // capability-routes table and its GnuPG home exist only on the host Brama
    // runs on, so every command built from that context has to run there. This
    // read must not point at the runner's own host, which is exactly the
    // failure `brama_identity_host` was added to prevent: registering a
    // repository-scoped runner on a host Brama does not run on answers
    // `cannot read Brama's Skarbiec path declarations: /usr/bin/grep:
    // /root/.config/brama/service.env: No such file or directory`.
    let identity = brama_identity_host(&target).await?;
    let context = brama_skarbiec_context(&identity).await?;
    reconcile_brama_introspection_grant(&identity, &context).await?;
    let primary_route = reconcile_model_review_route(&identity, &context).await?;
    let github_token = github_credential().await?;
    let client_id = model_review_client_id(repository);
    let capability = format!("call:brama#{MODEL_REVIEW_ALIAS}");
    let program_path = "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    let minted = host_channel::run_program(
        &identity,
        &[
            "/usr/bin/env",
            &context.vault,
            &context.routes,
            &context.gnupg,
            program_path,
            &context.skarbiec,
            "grant",
            "issue",
            &client_id,
            "--capabilities",
            &capability,
            "--audience",
            MODEL_REVIEW_AGENT_AUDIENCE,
            "--replace-capabilities",
            "--until-revoked",
        ],
        &context.runner,
    )
    .await?;
    if !minted.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: model review bearer mint failed: {}",
            identity.name,
            command_failure(&minted, "Skarbiec token mint failed")
        )));
    }
    let document: Value = serde_json::from_str(&minted.stdout).map_err(|error| {
        DeployError::unreachable(format!("Skarbiec token response is invalid: {error}"))
    })?;
    let token = document
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| {
            DeployError::unreachable("Skarbiec token response has no bearer".to_string())
        })?;
    verify_model_review_bearer(token).await?;
    set_repository_secret(repository, MODEL_REVIEW_SECRET, token, &github_token)?;
    Ok(json!({
        "target": target.name,
        "organization": GITHUB_ORGANIZATION,
        "repository": repository,
        "client_id": client_id,
        "model": MODEL_REVIEW_ALIAS,
        "primary_route": primary_route,
        "audience": MODEL_REVIEW_AGENT_AUDIENCE,
        "secret": MODEL_REVIEW_SECRET,
        "status": "reconciled",
    }))
}
