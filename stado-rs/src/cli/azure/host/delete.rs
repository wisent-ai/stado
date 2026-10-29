//! `stado cloud host delete NAME --provider azure`: return one cloud host —
//! its VM, disk, interface, security group and address on Azure, then its
//! registry entry and its fleet key.

use serde_json::json;

use super::declaration;
use super::{azure, Azure, DeleteArgs};
use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::providers::azure::cloud_host;

pub(super) async fn delete(args: DeleteArgs) -> Result<(), CmdError> {
    let super::super::CloudProvider::Azure = args.provider;
    let az = azure()?;
    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let declared = declaration::hosts(&document, super::create::PROVIDER)?;
    if !declared.iter().any(|host| host.name == args.name) {
        return Err(CmdError::missing(format!(
            "the registry declares no Azure cloud host {} (it declares {})",
            args.name,
            if declared.is_empty() {
                "none".to_string()
            } else {
                declared
                    .iter()
                    .map(|host| host.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )));
    }
    let deleted = remove(&az, &args.name).await?;
    let receipt = json!({ "target": args.name, "deleted": deleted });
    if args.json {
        crate::cli::print_answer(&receipt, true)?;
    } else {
        println!(
            "{} returned: {} deleted on Azure, registry entry and fleet key removed",
            args.name,
            deleted.join(", ")
        );
    }
    Ok(())
}

/// Delete host `name` on Azure, and only once nothing of it stands there,
/// its registry entry and its fleet key. A resource Azure still holds keeps
/// the entry, so the host stays listed until it is truly gone.
pub(super) async fn remove(az: &Azure, name: &str) -> Result<Vec<String>, CmdError> {
    let deleted = cloud_host::delete(&az.client, az.resource_group, name)
        .await
        .map_err(|standing| {
            CmdError::unreachable(format!(
                "{name} is still registered because Azure did not release everything: {}; deleting it again retries",
                standing.join("; ")
            ))
        })?;
    let name_owned = name.to_string();
    commit_document(move |document| crate::cli::fleet::ops::remove_target(document, &name_owned))
        .await?;
    forget_key(name).await?;
    Ok(deleted)
}

/// Delete host `name`'s fleet key item from the credential store.
pub(super) async fn forget_key(name: &str) -> Result<(), CmdError> {
    let item = crate::cli::fleet::key::item_id(name);
    crate::cli::fleet::key::configured_client()?
        .delete_item(&item)
        .await
        .map_err(|error| {
            CmdError::from(error).within(format!("the fleet key {item} of {name} was not removed"))
        })
}
