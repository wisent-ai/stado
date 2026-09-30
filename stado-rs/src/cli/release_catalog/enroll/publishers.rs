//! Every release publisher a product's build reads through: its own, and the
//! publisher of each pinned build input (`inputs.<name>.uri`).
//!
//! On 2026-09-30 oko-desktop and tama-desktop were refused at queue time with
//! 'cannot read release publisher item oko-desktop-swiftpm-cache … Skarbiec
//! returned HTTP 403' (34891aeb). The input publishers were declared in this
//! host's configuration, so `ensure_publisher` passed them, but their items
//! had been minted into this machine's retired local vault copy (c39eb66d) and
//! the owner vault held neither the item nor Stado's grant on it. An input's
//! publisher is ensured by what Stado can read: an item the vault refuses to
//! Stado (403) or does not hold (404, or no token) is declared again, which
//! mints it on the owner and grants the read. Any other failure of the read
//! refuses the enrolment, so an unreachable vault never rotates a bearer.

use reqwest::StatusCode;

use crate::cli::CmdError;
use crate::release_pipeline::ReleasePipelineManifest;

use super::super::publisher::{declare_publisher_on_fleet, ensure_publisher};

/// Ensure `manifest`'s own publisher and the publisher of every input it reads.
pub(super) async fn ensure_publishers(manifest: &ReleasePipelineManifest) -> Result<(), CmdError> {
    ensure_publisher(&manifest.product).await?;
    for (name, input) in &manifest.inputs {
        let Some(item) = input_publisher(&input.uri)? else {
            continue;
        };
        if readable(&item).await? {
            continue;
        }
        eprintln!(
            "{}: input {name} is published by {item}, which Stado cannot read; declaring it on \
             the vault owner",
            manifest.product
        );
        declare_publisher_on_fleet(&item).await?;
    }
    Ok(())
}

/// The publisher item a `stado://<namespace>/<key>` input is read with, or
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

/// Whether Stado's own grant reads a non-empty token from `item`, the read
/// `release_bearer_for` makes when the build stages the input. `false` only
/// for the vault's own answer that Stado may not read it or it is not there.
async fn readable(item: &str) -> Result<bool, CmdError> {
    let client = crate::skarbiec::Client::stado()
        .map_err(|error| CmdError::click(format!("cannot acquire Stado's grant: {error}")))?;
    match client.read_string(item, "token").await {
        Ok(token) => Ok(token.is_some_and(|token| !token.is_empty())),
        Err(error)
            if error.status() == Some(StatusCode::FORBIDDEN.as_u16())
                || error.status() == Some(StatusCode::NOT_FOUND.as_u16()) =>
        {
            Ok(false)
        }
        Err(error) => Err(CmdError::click(format!(
            "cannot check whether Stado reads publisher item {item}: {error}"
        ))),
    }
}
