//! The model-review bearer a repository's CI presents to Brama, and the route
//! and grant that make it answerable.

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::brama::{
    brama_identity_host, brama_skarbiec_context, BramaSkarbiecContext,
};
use crate::deploy::host_precheck_runner::accounts::github::{
    github_credential, repository_name, set_repository_secret, GITHUB_ORGANIZATION,
};
use crate::deploy::host_precheck_runner::declaration::runner_target;
use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{host_channel, shlex_quote, DeployError};
use crate::targets::ComputeTarget;

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

async fn reconcile_brama_introspection_grant(
    target: &ComputeTarget,
    context: &BramaSkarbiecContext,
) -> Result<(), DeployError> {
    let token_file = format!("{}/.stado/{BRAMA_INTROSPECTION_TOKEN_FILE}", context.home);
    let script = format!(
        "set -eu\n\
         PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin; export PATH\n\
         export {}\n\
         export {}\n\
         export {}\n\
         token_file={}\n\
         if [ -L \"$token_file\" ]; then\n\
           printf '%s\\n' 'Brama introspection bearer must not be a symlink' >&2\n\
           exit 40\n\
         fi\n\
         if [ -f \"$token_file\" ]; then\n\
           /bin/chmod 600 \"$token_file\"\n\
           {} grant issue {} --capabilities {} --replace-capabilities \
             --token-file \"$token_file\" --until-revoked >/dev/null\n\
         else\n\
           staged=\"$token_file.stado-new.$$\"\n\
           trap '/bin/rm -f \"$staged\"' EXIT HUP INT TERM\n\
           umask 077\n\
           /usr/bin/openssl rand -hex 32 > \"$staged\"\n\
           {} grant issue {} --capabilities {} --replace-capabilities \
             --token-file \"$staged\" --until-revoked >/dev/null\n\
           /bin/mv -f \"$staged\" \"$token_file\"\n\
           trap - EXIT HUP INT TERM\n\
         fi\n",
        shlex_quote(&context.vault),
        shlex_quote(&context.routes),
        shlex_quote(&context.gnupg),
        shlex_quote(&token_file),
        shlex_quote(&context.skarbiec),
        BRAMA_INTROSPECTION_CONSUMER,
        BRAMA_INTROSPECTION_CAPABILITY,
        shlex_quote(&context.skarbiec),
        BRAMA_INTROSPECTION_CONSUMER,
        BRAMA_INTROSPECTION_CAPABILITY,
    );
    let reconciled = host_channel::run_script(target, &script, &context.runner).await?;
    if !reconciled.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: Brama introspection grant reconciliation failed: {}",
            target.name,
            command_failure(&reconciled, "Skarbiec introspection grant failed")
        )));
    }
    Ok(())
}

async fn reconcile_model_review_route(
    target: &ComputeTarget,
    context: &BramaSkarbiecContext,
) -> Result<String, DeployError> {
    let program_path = "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    let read = host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            &context.vault,
            &context.routes,
            &context.gnupg,
            program_path,
            &context.skarbiec,
            "get",
            BRAMA_DESKTOP_MODEL_ROUTER_ROLE,
            "--field",
            "token",
        ],
        &context.runner,
    )
    .await?;
    if !read.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: Brama route administrator bearer read failed: {}",
            target.name,
            command_failure(&read, "Skarbiec route administrator read failed")
        )));
    }
    let token = read.stdout.trim();
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(DeployError(
            "Brama route administrator bearer is empty or malformed".to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Auth));
    }
    let origin = brama_origin()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("Brama route client failed")
        })?;
    let response = client
        .put(format!("{origin}/v1/admin/routes"))
        .bearer_auth(token)
        .json(&model_review_route_request())
        .send()
        .await
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("Brama route reconciliation failed")
        })?;
    if !response.status().is_success() {
        return Err(DeployError(format!(
            "Brama refused the model-review route reconciliation with HTTP {}",
            response.status().as_u16()
        ))
        .stating(crate::primitives::failure::FailureCode::from_upstream_status(
            response.status().as_u16(),
        )));
    }
    Ok(MODEL_REVIEW_PRIMARY_ROUTE.to_string())
}

/// The Brama address this machine dials: the forward marker the service
/// directory published, refused by name when there is none.
fn brama_origin() -> Result<String, DeployError> {
    let marker = crate::deploy::host_access::forward::read_local(BRAMA_SERVICE)?;
    let url = marker
        .map(|marker| marker.url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty());
    url.ok_or_else(|| {
        DeployError(format!(
            "no address for service {BRAMA_SERVICE:?} on this machine: run `stado service directory publish` so the directory writes ~/.stado/forwards/{BRAMA_SERVICE}.local"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })
}

async fn verify_model_review_bearer(token: &str) -> Result<(), DeployError> {
    let origin = brama_origin()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("Brama verification client failed")
        })?;
    let response = client
        .get(format!("{origin}/v1/models"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("Brama bearer verification failed")
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(DeployError(format!(
            "Brama refused the newly minted model-review bearer with HTTP {}",
            status.as_u16()
        ))
        .stating(crate::primitives::failure::FailureCode::from_upstream_status(
            status.as_u16(),
        )));
    }
    let catalog: Value = response.json().await.map_err(|error| {
        DeployError::unreachable(format!("Brama model catalog is invalid: {error}"))
    })?;
    let route_advertised = catalog
        .get("data")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("id").and_then(Value::as_str) == Some(MODEL_REVIEW_ALIAS))
        });
    if !route_advertised {
        return Err(DeployError(format!(
            "Brama did not advertise the model-review route {MODEL_REVIEW_ALIAS}"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    Ok(())
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
