//! The check that runs before any capability exists: that the redeeming host
//! routes both sign-in resources to the item the caller named.

use super::super::SIGN_IN_FIELDS;
use super::fill_resource;
use super::refusal::missing_route_sentence;
use super::table::{routed_item, RoutedField};
use crate::deploy::{host_capability, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Confirm the redeeming host routes both resources to the item the caller
/// named.
///
/// Returns what each resource routes to, in `SIGN_IN_FIELDS` order: the vault
/// field decides which registered identity the capability must be issued to,
/// and `readable` is the part the caller says out loud. Whether the broker can
/// open the item is the broker's business at
/// redemption; whether the route points at the item the operator named is this
/// command's business, and that is what is enforced here.
pub async fn confirm_routed_item(
    target: &ComputeTarget,
    broker: &host_capability::RemoteBroker,
    origin: &str,
    item: &str,
    runner: &Runner,
) -> Result<Vec<RoutedField>, DeployError> {
    let routes = host_capability::routes(target, broker, runner).await?;
    let mut confirmed = Vec::with_capacity(SIGN_IN_FIELDS.len());
    for (_, field_class) in SIGN_IN_FIELDS {
        let resource = fill_resource(origin, field_class);
        let routed = routed_item(&routes, &resource).map_err(|error| {
            DeployError(missing_route_sentence(
                &target.name,
                origin,
                item,
                &error.to_string(),
            ))
        })?;
        if routed.item != item {
            return Err(DeployError(format!(
                "{}: {resource} routes to vault item {} field {}, not to {item}; \
                 the item that would be read is the one the route names",
                target.name, routed.item, routed.field
            )));
        }
        confirmed.push(routed);
    }
    Ok(confirmed)
}
