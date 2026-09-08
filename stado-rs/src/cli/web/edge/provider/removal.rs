//! Undoing a provision: deleting what it created, in the order Azure accepts,
//! and forgetting the declaration last.

use serde_json::json;

use super::super::declaring::declared;
use super::super::serving::stado_routes;
use super::requests::network_path;
use super::{discard, CmdError};
use crate::config;
use crate::providers::azure;

/// Undo [`super::provision::provision`], and be the one command that does.
///
/// Provisioning the edge is the only thing in this capability that spends
/// money, so the command that reverses it is part of the contract rather than
/// an operator's Azure console session. It deletes in reverse creation order
/// through the same [`discard`] the rollback uses — VM, NIC, security group,
/// public address — because a public address cannot be released while an
/// interface still references it, and an address left behind is billed while
/// belonging to nothing.
///
/// It refuses while a product still names this edge. Those hostnames' A
/// records point at the address this command is about to release, so deleting
/// it first is an outage that outlives the command: the record stays, the
/// address goes back to Azure's pool, and the name resolves to whatever gets
/// it next. `--orphan-hostnames` is how an operator says that is understood.
///
/// `--keep-resources` forgets the declaration and touches nothing on Azure.
/// That is the correct removal for an edge recorded with
/// [`super::super::declaring::declare`] rather
/// than created here: Stado did not make those resources and must not delete
/// them.
pub(in crate::cli::web::edge) async fn remove(
    orphan_hostnames: bool,
    keep_resources: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let edge = declared()?;
    let routes = stado_routes().await.unwrap_or_default();
    if !routes.is_empty() && !orphan_hostnames {
        return Err(CmdError::usage(format!(
            "{} still terminates {} hostname(s) — {} — whose A records point at {}. Retract them \
             with `stado web remove <product>` first, or pass --orphan-hostnames to delete the \
             edge anyway and leave those records pointing at an address Azure will give to \
             somebody else.",
            edge.target(),
            routes.len(),
            routes
                .iter()
                .map(|(hostname, _)| hostname.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            edge.address(),
        )));
    }

    let mut deleted: Vec<String> = Vec::new();
    let mut leftovers: Vec<String> = Vec::new();
    if !keep_resources {
        let subscription = config::azure_subscription_id();
        if subscription.is_empty() {
            return Err(CmdError::click(
                "AZURE_SUBSCRIPTION_ID is empty, so no Azure resource can be addressed; set it, \
                 or pass --keep-resources to forget the declaration only",
            ));
        }
        let resource_group = config::azure_resource_group();
        if resource_group.is_empty() {
            return Err(CmdError::click(
                "AZURE_RESOURCE_GROUP is empty, so there is no resource group to delete the edge \
                 from; set it, or pass --keep-resources to forget the declaration only",
            ));
        }
        let name = edge.target();
        let client = azure::ArmClient::new(subscription);
        // Reverse creation order, and each one waited out: the next delete in
        // the list is refused by Azure while the previous resource still
        // references it.
        let targets = [
            (
                format!(
                    "{}?api-version={}",
                    azure::vm_path(subscription, resource_group, name),
                    azure::COMPUTE_API_VERSION
                ),
                format!("VM {name}"),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "networkInterfaces",
                    &azure::network::nic_name(name),
                ),
                format!("NIC {}", azure::network::nic_name(name)),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "networkSecurityGroups",
                    &format!("{name}-nsg"),
                ),
                format!("security group {name}-nsg"),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "publicIPAddresses",
                    &format!("{name}-ip"),
                ),
                format!("public IP {name}-ip"),
            ),
        ];
        for (path, description) in &targets {
            match discard(&client, path, description).await {
                Ok(()) => deleted.push(description.clone()),
                Err(problem) => leftovers.push(problem),
            }
        }
    }

    // The declaration goes last. While it is still there `stado web edge
    // status` names the host an operator has to finish cleaning up; dropped
    // first, a half-failed delete would leave resources nothing points at.
    if !leftovers.is_empty() {
        return Err(CmdError::click(format!(
            "the edge declaration was kept because Azure did not release everything: {}. Deleted: \
             {}. Re-run this command once those are gone.",
            leftovers.join("; "),
            if deleted.is_empty() {
                "nothing".to_string()
            } else {
                deleted.join(", ")
            }
        )));
    }
    super::mutate_web("edge", |map| {
        map.clear();
        Ok(())
    })?;
    let report = json!({
        "target": edge.target(),
        "address": edge.address(),
        "deleted": deleted,
        "orphaned_hostnames": routes
            .iter()
            .map(|(hostname, _)| hostname.clone())
            .collect::<Vec<_>>(),
        "change": "removed",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} removed as the fleet's web edge; {} deleted on Azure",
            edge.target(),
            if deleted.is_empty() {
                "nothing".to_string()
            } else {
                deleted.join(", ")
            }
        );
    }
    Ok(())
}
