//! Every release publisher a product's build reads through: its own, and the
//! publisher of each pinned build input (`inputs.<name>.uri`).
//!
//! An input's publisher is ensured by what Stado can read: the item
//! `release_api.publishers` declares for it, read as named, exactly as the
//! build reads it (`release_bearer_for`). An item the vault refuses to Stado
//! (403) or does not hold (404, or no token) is declared again, which stores
//! the bearer on the owner and grants the read. Any other failure of the read
//! refuses the enrolment, so an unreachable vault never rotates a bearer.
//!
//! Checking by role instead found no item on a vault whose Skarbiec predates
//! role tags, declared a publisher the build could already read, and the
//! declaration's role tag was refused by that vault, so the Skarbiec release
//! that would have added role tags could never be submitted.

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
        // The configuration reader validates that each publisher's item is
        // its product name and its prefix is exactly `<product>/`.
        eprintln!(
            "{}: input {name} is published by {item}, whose token Stado cannot read; \
             declaring {item}'s publisher on the vault owner",
            manifest.product
        );
        declare_publisher_on_fleet(&item).await?;
    }
    Ok(())
}

/// The publisher item `release_api.publishers` declares for a
/// `stado://<namespace>/<key>` input, or `None` for an input no release
/// publisher covers.
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

/// Whether Stado's own grant reads a non-empty token from the publisher item
/// `item` as declared, the read `release_bearer_for` makes when the build
/// stages the input. `false` only for the vault's own answer that Stado may
/// not read it or does not hold it.
async fn readable(item: &str) -> Result<bool, CmdError> {
    let client = crate::skarbiec::Client::stado().map_err(|error| {
        CmdError::click(format!("cannot acquire Stado's grant: {error}"))
            .stating(error.failure_code())
    })?;
    match client.read_declared_string(item, "token").await {
        Ok(token) => Ok(token.is_some_and(|token| !token.is_empty())),
        Err(error)
            if error.status() == Some(StatusCode::FORBIDDEN.as_u16())
                || error.status() == Some(StatusCode::NOT_FOUND.as_u16()) =>
        {
            Ok(false)
        }
        Err(error) => Err(CmdError::click(format!(
            "cannot check whether Stado reads publisher item {item}: {error}"
        ))
        .stating(error.failure_code())),
    }
}
