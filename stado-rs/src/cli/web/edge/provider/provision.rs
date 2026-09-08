//! Creating the edge: its public address, its security group, its interface,
//! the VM itself, and the declaration that records them.

use serde_json::{json, Value};

use super::super::declaring::{checked_declaration, record};
use super::super::{EDGE_CLOUD_INIT, EDGE_DISK_GB, EDGE_IMAGE_URN, PROXY_UNIT};
use super::requests::{interface_body, network_path, public_ip_body, security_group_body};
use super::{refusal, unwind, CmdError};
use crate::config;
use crate::providers::azure;

pub(in crate::cli::web::edge) async fn provision(
    name: &str,
    region: &str,
    size: &str,
    contact: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    checked_declaration(name, contact)?;
    let subscription = config::azure_subscription_id();
    if subscription.is_empty() {
        return Err(CmdError::click(
            "AZURE_SUBSCRIPTION_ID is empty, so no Azure resource can be addressed; set it in \
             Stado's configuration before provisioning an edge",
        ));
    }
    let resource_group = config::azure_resource_group();
    if resource_group.is_empty() {
        return Err(CmdError::click(
            "AZURE_RESOURCE_GROUP is empty, so there is no resource group to create the edge in",
        ));
    }
    let ssh_public_key = config::azure_ssh_public_key();
    if ssh_public_key.is_empty() {
        // The rendered VM body sets disablePasswordAuthentication, so Azure
        // refuses a Linux VM with no key at all. Refusing here names the
        // configuration key instead of returning an ARM validation error.
        return Err(CmdError::click(
            "AZURE_SSH_PUBLIC_KEY is empty, and the edge is rendered with \
             disablePasswordAuthentication, so Azure would refuse a VM with no way in at all; \
             set it before provisioning an edge",
        ));
    }

    let client = azure::ArmClient::new(subscription);
    let interface_name = azure::network::nic_name(name);
    let address_name = format!("{name}-ip");
    let group_name = format!("{name}-nsg");
    let address_path = network_path(
        subscription,
        resource_group,
        "publicIPAddresses",
        &address_name,
    );
    let group_path = network_path(
        subscription,
        resource_group,
        "networkSecurityGroups",
        &group_name,
    );
    let interface_path = network_path(
        subscription,
        resource_group,
        "networkInterfaces",
        &interface_name,
    );
    // Reverse creation order is the rollback order, so this list is the order
    // resources are appended to it.
    let mut created: Vec<(String, String)> = Vec::new();

    client
        .put_lro(
            &address_path,
            &public_ip_body(region, name),
            &format!("create public IP {address_name}"),
        )
        .await
        .map_err(|error| refusal("the edge's public address was not created", error, &[]))?;
    created.push((address_path.clone(), format!("public IP {address_name}")));

    let allocated = match client
        .get(&address_path, &format!("read public IP {address_name}"))
        .await
    {
        Ok(allocated) => allocated,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's public address was created and could not be read back",
                error,
                &leftovers,
            ));
        }
    };
    let address = allocated
        .pointer("/properties/ipAddress")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        let leftovers = unwind(&client, &created).await;
        return Err(refusal(
            "the edge's public address was created without an IPv4 address, so no A record could \
             ever point at it",
            format!("Azure reported {address:?}"),
            &leftovers,
        ));
    }
    let address_id = allocated
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let group = match client
        .put_lro(
            &group_path,
            &security_group_body(region, name),
            &format!("create network security group {group_name}"),
        )
        .await
    {
        Ok(group) => group,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's security group was not created, so 80 and 443 could not be opened",
                error,
                &leftovers,
            ));
        }
    };
    created.push((group_path.clone(), format!("security group {group_name}")));
    let group_id = group
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let subnet_id = azure::network::subnet_id(
        subscription,
        resource_group,
        config::azure_vnet(),
        config::azure_subnet(),
        region,
    );
    let interface = match client
        .put_lro(
            &interface_path,
            &interface_body(region, &subnet_id, &group_id, &address_id),
            &format!("create NIC {interface_name}"),
        )
        .await
    {
        Ok(interface) => interface,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's network interface was not created; a region without a \
                 pre-provisioned vnet and subnet is the usual reason",
                error,
                &leftovers,
            ));
        }
    };
    created.push((interface_path.clone(), format!("NIC {interface_name}")));
    let interface_id = interface
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // No managed identity: the edge runs a reverse proxy, not an agent, so it
    // needs no token source for the blob queue and never deletes itself. Not
    // preemptible either — an evicted edge takes every hostname it terminates
    // off the internet.
    let body = azure::vm_body(
        name,
        region,
        size,
        EDGE_DISK_GB,
        EDGE_IMAGE_URN,
        config::azure_vm_username(),
        ssh_public_key,
        EDGE_CLOUD_INIT,
        &interface_id,
        "",
        false,
    )
    .map_err(CmdError::click)?;
    let machine_path = format!(
        "{}?api-version={}",
        azure::vm_path(subscription, resource_group, name),
        azure::COMPUTE_API_VERSION
    );
    if let Err(error) = client
        .put_lro(&machine_path, &body, &format!("create VM {name}@{region}"))
        .await
    {
        let leftovers = unwind(&client, &created).await;
        return Err(refusal(
            "the edge VM was not created; an ARM64 image with an x86-64 --size, and a size with \
             no capacity in the region, are the two usual reasons",
            error,
            &leftovers,
        ));
    }

    let change = record(name, &address, contact)?;
    let report = json!({
        "target": name,
        "address": address,
        "contact": contact,
        "region": region,
        "size": size,
        "image": EDGE_IMAGE_URN,
        "subscription": subscription,
        "resource_group": resource_group,
        "public_ip": address_name,
        "security_group": group_name,
        "network_interface": interface_name,
        "change": change,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{name} at {address} in {region} ({size}): {change} as the fleet's web edge");
        println!(
            "next: join {name} to the tailnet and the registry, then install its reverse proxy \
             with `stado service deploy {} --host {name}`",
            super::unit_label(PROXY_UNIT)
        );
    }
    Ok(())
}
