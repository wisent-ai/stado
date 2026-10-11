//! What the model-review reconciliation asks of Brama and its vault: the
//! introspection grant, the model-review route, and the minted bearer's
//! proof against Brama's model catalog.

use serde_json::Value;

use crate::deploy::host_precheck_runner::accounts::brama::BramaSkarbiecContext;
use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{host_channel, shlex_quote, DeployError};
use crate::targets::ComputeTarget;

use super::{
    model_review_route_request, BRAMA_DESKTOP_MODEL_ROUTER_ROLE, BRAMA_INTROSPECTION_CAPABILITY,
    BRAMA_INTROSPECTION_CONSUMER, BRAMA_INTROSPECTION_TOKEN_FILE, BRAMA_SERVICE,
    MODEL_REVIEW_ALIAS, MODEL_REVIEW_PRIMARY_ROUTE,
};

/// Issue, until revoked, the grant Brama introspects bearers with on the
/// vault beside it, minting its bearer file when there is none.
pub(super) async fn reconcile_brama_introspection_grant(
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

/// Point the model-review alias at its one route through Brama's admin API,
/// with the route administrator bearer the host's vault holds.
pub(super) async fn reconcile_model_review_route(
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
    let response = crate::wait::request(
        client
            .put(format!("{origin}/v1/admin/routes"))
            .bearer_auth(token)
            .json(&model_review_route_request()),
    )
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
        .stating(
            crate::primitives::failure::FailureCode::from_upstream_status(
                response.status().as_u16(),
            ),
        ));
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

/// Prove the newly minted bearer opens Brama and that Brama advertises the
/// model-review route to it.
pub(super) async fn verify_model_review_bearer(token: &str) -> Result<(), DeployError> {
    let origin = brama_origin()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("Brama verification client failed")
        })?;
    let response =
        crate::wait::request(client.get(format!("{origin}/v1/models")).bearer_auth(token))
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
        .stating(crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16())));
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
