//! Every release publisher a product's build reads through: its own, and the
//! publisher of each pinned build input (`inputs.<name>.uri`).
//!
//! An input's publisher is ensured by what Stado can read: a role whose item
//! the vault refuses to Stado (403) or does not hold (404, or no token) is
//! declared again, which stores the bearer on the owner and grants the read.
//! Any other failure of the read refuses the enrolment, so an unreachable
//! vault never rotates a bearer.

use reqwest::StatusCode;

use crate::cli::CmdError;
use crate::release_pipeline::ReleasePipelineManifest;

use super::super::publisher::{declare_publisher_on_fleet, ensure_publisher, publisher_product};

/// Ensure `manifest`'s own publisher and the publisher of every input it reads.
pub(super) async fn ensure_publishers(manifest: &ReleasePipelineManifest) -> Result<(), CmdError> {
    ensure_publisher(&manifest.product).await?;
    for (name, input) in &manifest.inputs {
        let Some(role) = input_publisher(&input.uri)? else {
            continue;
        };
        if readable(&role).await? {
            continue;
        }
        let product = publisher_product(&role).ok_or_else(|| {
            CmdError::click(format!(
                "{}: input {name} is published under role {role}, which is not a release \
                 publisher role",
                manifest.product
            ))
        })?;
        eprintln!(
            "{}: input {name} is published under role {role}, which Stado cannot read; \
             declaring {product}'s publisher on the vault owner",
            manifest.product
        );
        declare_publisher_on_fleet(product).await?;
    }
    Ok(())
}

/// The publisher role a `stado://<namespace>/<key>` input is read with, or
/// `None` for an input no release publisher covers.
fn input_publisher(uri: &str) -> Result<Option<String>, CmdError> {
    let Some((namespace, key)) = uri
        .strip_prefix("stado://")
        .and_then(|rest| rest.split_once('/'))
    else {
        return Ok(None);
    };
    let Some(policy_key) = crate::remote::object_store::release_policy_key(namespace, key) else {
        return Ok(None);
    };
    let publisher =
        crate::config::release_client_publisher_for_key(&policy_key).map_err(|problems| {
            CmdError::click(format!(
                "release_api.publishers is invalid: {}",
                problems.join("; ")
            ))
        })?;
    Ok(publisher.map(|publisher| publisher.item().to_owned()))
}

/// Whether Stado's own grant reads a non-empty token from the item playing
/// `role`, the read `release_bearer_for` makes when the build stages the
/// input. `false` only for the vault's own answer that Stado may not read it
/// or no item plays it.
async fn readable(role: &str) -> Result<bool, CmdError> {
    let client = crate::skarbiec::Client::stado()
        .map_err(|error| CmdError::click(format!("cannot acquire Stado's grant: {error}")))?;
    match client.read_string(role, "token").await {
        Ok(token) => Ok(token.is_some_and(|token| !token.is_empty())),
        Err(error)
            if error.status() == Some(StatusCode::FORBIDDEN.as_u16())
                || error.status() == Some(StatusCode::NOT_FOUND.as_u16()) =>
        {
            Ok(false)
        }
        Err(error) => Err(CmdError::click(format!(
            "cannot check whether Stado reads publisher role {role}: {error}"
        ))),
    }
}
