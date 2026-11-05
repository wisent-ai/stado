//! Opening the edge's path from the Internet: every security group between
//! it and its interface that drops 80 or 443 gets an allow to the edge's own
//! private address.

use serde_json::json;

use super::CmdError;
use crate::config;
use crate::providers::azure;
use crate::providers::azure::layers::{self, Path};

/// The ports the edge serves: HTTP for Let's Encrypt's challenge and the
/// redirect, HTTPS for every hostname
/// (<https://www.rfc-editor.org/rfc/rfc9110#section-4.2>).
pub(in crate::cli::web::edge) const EDGE_PORTS: [u16; 2] = [80, 443];

/// The Azure client and resource group every edge network call addresses,
/// or the configuration that is missing.
pub(in crate::cli::web::edge) fn azure_scope() -> Result<(azure::ArmClient, &'static str), CmdError>
{
    let subscription = config::azure_subscription_id();
    if subscription.is_empty() {
        return Err(CmdError::click(
            "AZURE_SUBSCRIPTION_ID is empty, so the edge's security groups cannot be read; set it \
             in Stado's configuration",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    let resource_group = config::azure_resource_group();
    if resource_group.is_empty() {
        return Err(CmdError::click(
            "AZURE_RESOURCE_GROUP is empty, so there is no resource group holding the edge's \
             interface",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok((azure::ArmClient::new(subscription), resource_group))
}

/// The path from the Internet to edge `name`'s interface, refused when Azure
/// holds no such interface.
pub(in crate::cli::web::edge) async fn edge_path(
    client: &azure::ArmClient,
    resource_group: &str,
    name: &str,
) -> Result<Path, CmdError> {
    let interface = azure::network::nic_name(name);
    match layers::read(client, resource_group, &interface, &EDGE_PORTS).await {
        Ok(Some(path)) => Ok(path),
        Ok(None) => Err(CmdError::refused(format!(
            "Azure holds no interface {interface} in {resource_group}: {name} was not provisioned \
             by `stado web edge provision`, or it was removed"
        ))),
        Err(cause) => Err(CmdError::refused(cause)),
    }
}

/// Add the allows `path` lacks, and read the path again to show they hold.
pub(in crate::cli::web::edge) async fn open_path(
    client: &azure::ArmClient,
    resource_group: &str,
    name: &str,
    path: &Path,
) -> Result<(Vec<String>, Path), CmdError> {
    let written = layers::open(client, resource_group, name, path)
        .await
        .map_err(CmdError::refused)?;
    let after = edge_path(client, resource_group, name).await?;
    let still: Vec<String> = after
        .layers
        .iter()
        .filter(|layer| !layer.drops().is_empty())
        .map(|layer| format!("{} drops {:?}", layer.group_name(), layer.drops()))
        .collect();
    if !still.is_empty() {
        return Err(CmdError::refused(format!(
            "the allows were written ({}) and the path still drops: {}; a rule of higher \
             priority denies the port",
            written.join(", "),
            still.join("; ")
        )));
    }
    Ok((written, after))
}

pub(in crate::cli::web::edge) async fn repair(
    name: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (client, resource_group) = azure_scope()?;
    let before = edge_path(&client, resource_group, name).await?;
    let (written, after) = open_path(&client, resource_group, name, &before).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "edge": name,
                "written": written,
                "before": before.report(),
                "after": after.report(),
            }))?
        );
        return Ok(());
    }
    if written.is_empty() {
        println!(
            "{name}: every security group already allows 80 and 443 to {}; nothing written",
            after.private_address
        );
    } else {
        println!(
            "{name}: allowed to {} only: {}",
            after.private_address,
            written.join(", ")
        );
    }
    Ok(())
}
