//! A cloud host on Azure: a VM Stado rents to be a fleet host, reached over
//! SSH at its own public address, and the admission that lets exactly the
//! fleet's own hosts reach it.
//!
//! Each host is its own public address, its own security group, its
//! interface and its VM, every one tagged with the host's name; deleting the
//! host deletes all four. The subnets the deployment declares
//! (`AZURE_VNET`-`<location>`/`AZURE_SUBNET`) may carry a security group of
//! their own, which a subnet-level group applies before the interface's, so
//! the location also gets one application security group
//! (`stado-cloud-hosts-<location>`) and, in the subnet's group, one rule of
//! the same name admitting the fleet's addresses to that application group
//! only. Every other interface in the subnet stays as it was.

use serde_json::{json, Value};

use super::public::{
    application_security_group_body, discard, free_priority, inbound_rule_properties,
    interface_body, network_path, public_ip_body, security_group_body, tags, unwind, Inbound,
};
use super::{vm_body, vm_path, ArmClient, COMPUTE_API_VERSION, NETWORK_API_VERSION};

/// The `wisent_role` tag every resource of a cloud host carries.
pub const ROLE_TAG: &str = "cloud-host";
/// The name of the admission rule in a subnet's security group, and the
/// prefix of the application security group it admits to.
const ADMISSION: &str = "stado-cloud-hosts";
/// The `wisent_role` tag of the per-location application security group.
const ADMISSION_ROLE_TAG: &str = "cloud-host-admission";
/// The name of the SSH rule in a host's own security group.
const SSH_RULE: &str = "allow-ssh-from-fleet";
/// SSH's IANA port, the one the image's sshd listens on.
pub const SSH_PORT: &str = "22";

/// What one host is created from: every value is the caller's.
pub struct Spec<'a> {
    pub name: &'a str,
    pub location: &'a str,
    pub size: &'a str,
    pub disk_gb: i64,
    pub image_urn: &'a str,
    pub username: &'a str,
    pub public_key: &'a str,
    pub custom_data: &'a str,
    pub subnet_vnet: &'a str,
    pub subnet: &'a str,
}

/// What a create left standing: the public address and the VM's resource id.
pub struct Created {
    pub address: String,
    pub vm_id: String,
}

fn address_name(name: &str) -> String {
    format!("{name}-ip")
}

fn group_name(name: &str) -> String {
    format!("{name}-nsg")
}

fn admission_group_name(location: &str) -> String {
    format!("{ADMISSION}-{location}")
}

fn vm_api_path(client: &ArmClient, rg: &str, name: &str) -> String {
    format!(
        "{}?api-version={COMPUTE_API_VERSION}",
        vm_path(client.subscription(), rg, name)
    )
}

/// The ARM path, with its API version, of a resource whose id ARM stated.
fn id_path(id: &str, child: &str) -> String {
    format!("{id}{child}?api-version={NETWORK_API_VERSION}")
}

/// Make `sources` the addresses admitted to the cloud hosts of `location`:
/// the location's application security group exists, and the security group
/// of the declared subnet, when it carries one, admits exactly `sources` to
/// it on SSH. Returns the application group's id, which a host's interface
/// joins.
pub async fn admit(
    client: &ArmClient,
    rg: &str,
    vnet: &str,
    subnet: &str,
    location: &str,
    sources: &[String],
) -> Result<String, String> {
    let subscription = client.subscription();
    let group_name = admission_group_name(location);
    let group_path = network_path(subscription, rg, "applicationSecurityGroups", &group_name);
    let group = client
        .put_lro(
            &group_path,
            &application_security_group_body(location, &tags(ADMISSION_ROLE_TAG, location)),
            &format!("create application security group {group_name}"),
        )
        .await
        .map_err(|error| {
            format!("the application security group {group_name} was not made: {error}")
        })?;
    let Some(group_id) = group.get("id").and_then(Value::as_str) else {
        return Err(format!(
            "Azure made the application security group {group_name} but its answer names no id: {group}"
        ));
    };
    let subnet_path = format!(
        "{}?api-version={NETWORK_API_VERSION}",
        super::network::subnet_id(subscription, rg, vnet, subnet, location)
    );
    let declared = client
        .get(&subnet_path, &format!("read subnet {vnet}-{location}/{subnet}"))
        .await
        .map_err(|error| {
            format!(
                "the subnet {subnet} of {vnet}-{location} the deployment declares (AZURE_VNET, AZURE_SUBNET) could not be read: {error}"
            )
        })?;
    if let Some(subnet_group) = declared
        .pointer("/properties/networkSecurityGroup/id")
        .and_then(Value::as_str)
    {
        let current = client
            .get(
                &id_path(subnet_group, ""),
                &format!("read security group {subnet_group}"),
            )
            .await
            .map_err(|error| {
                format!("the subnet's security group {subnet_group} could not be read: {error}")
            })?;
        let priority = free_priority(&current, ADMISSION)?;
        let destination = [group_id.to_string()];
        let rule = Inbound {
            name: ADMISSION,
            port: SSH_PORT,
            sources,
            destination_groups: &destination,
            priority,
        };
        client
            .put_lro(
                &id_path(subnet_group, &format!("/securityRules/{ADMISSION}")),
                &json!({ "properties": inbound_rule_properties(&rule) }),
                &format!("admit the fleet's addresses in {subnet_group}"),
            )
            .await
            .map_err(|error| {
                format!("the rule {ADMISSION} admitting the fleet's addresses was not written to {subnet_group}: {error}")
            })?;
    }
    Ok(group_id.to_string())
}

/// Create host `spec.name`: its public address, its security group admitting
/// `sources` on SSH, its interface in the declared subnet and the
/// application group `admission`, and the VM. A failure removes what this
/// call created, and names whatever could not be removed.
pub async fn create(
    client: &ArmClient,
    rg: &str,
    spec: &Spec<'_>,
    sources: &[String],
    admission: &str,
) -> Result<Created, String> {
    let subscription = client.subscription();
    let name = spec.name;
    let location = spec.location;
    let host_tags = tags(ROLE_TAG, name);
    let mut created: Vec<(String, String)> = Vec::new();
    let failed = |created: Vec<(String, String)>, context: String| async move {
        let leftovers = unwind(client, &created).await;
        if leftovers.is_empty() {
            context
        } else {
            format!(
                "{context}; these resources were created and could not be removed, so they are still billing: {}",
                leftovers.join("; ")
            )
        }
    };

    let address_path = network_path(subscription, rg, "publicIPAddresses", &address_name(name));
    let address = match client
        .put_lro(
            &address_path,
            &public_ip_body(location, &host_tags),
            &format!("create public IP {}", address_name(name)),
        )
        .await
    {
        Ok(address) => address,
        Err(error) => {
            return Err(format!(
                "the public address of {name} was not created: {error}"
            ))
        }
    };
    created.push((
        address_path.clone(),
        format!("public IP {}", address_name(name)),
    ));
    let (address_id, address_text) = match (
        address.get("id").and_then(Value::as_str),
        client
            .get(&address_path, &format!("read public IP {}", address_name(name)))
            .await
            .map(|read| read.pointer("/properties/ipAddress").and_then(Value::as_str).map(str::to_string)),
    ) {
        (Some(id), Ok(Some(text))) if text.parse::<std::net::Ipv4Addr>().is_ok() => (id.to_string(), text),
        (id, read) => {
            return Err(failed(
                created,
                format!("the public address of {name} was created but names no id ({id:?}) or no IPv4 address ({read:?})"),
            )
            .await)
        }
    };

    let group_path = network_path(subscription, rg, "networkSecurityGroups", &group_name(name));
    let rules = [Inbound {
        name: SSH_RULE,
        port: SSH_PORT,
        sources,
        destination_groups: &[],
        priority: super::public::LOWEST_RULE_PRIORITY,
    }];
    let group = match client
        .put_lro(
            &group_path,
            &security_group_body(location, &host_tags, &rules),
            &format!("create network security group {}", group_name(name)),
        )
        .await
    {
        Ok(group) => group,
        Err(error) => {
            return Err(failed(
                created,
                format!("the security group of {name} was not created: {error}"),
            )
            .await)
        }
    };
    created.push((
        group_path.clone(),
        format!("security group {}", group_name(name)),
    ));
    let Some(group_id) = group.get("id").and_then(Value::as_str).map(str::to_string) else {
        return Err(failed(
            created,
            format!("the security group of {name} names no id: {group}"),
        )
        .await);
    };

    let interface_name = super::network::nic_name(name);
    let interface_path = network_path(subscription, rg, "networkInterfaces", &interface_name);
    let subnet =
        super::network::subnet_id(subscription, rg, spec.subnet_vnet, spec.subnet, location);
    let interface = match client
        .put_lro(
            &interface_path,
            &interface_body(
                location,
                &subnet,
                &group_id,
                &address_id,
                &[admission.to_string()],
                &host_tags,
            ),
            &format!("create NIC {interface_name}"),
        )
        .await
    {
        Ok(interface) => interface,
        Err(error) => {
            return Err(failed(
                created,
                format!("the interface of {name} was not created: {error}"),
            )
            .await)
        }
    };
    created.push((interface_path.clone(), format!("NIC {interface_name}")));
    let Some(interface_id) = interface
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Err(failed(
            created,
            format!("the interface of {name} names no id: {interface}"),
        )
        .await);
    };

    let mut body = match vm_body(
        name,
        location,
        spec.size,
        spec.disk_gb,
        spec.image_urn,
        spec.username,
        spec.public_key,
        spec.custom_data,
        &interface_id,
        "",
        false,
    ) {
        Ok(body) => body,
        Err(error) => return Err(failed(created, error).await),
    };
    if let (Some(body_tags), Some(host)) = (body["tags"].as_object_mut(), host_tags.as_object()) {
        body_tags.extend(host.clone());
    }
    let machine_path = vm_api_path(client, rg, name);
    let machine = match client
        .put_lro(
            &machine_path,
            &body,
            &format!("create VM {name}@{location}"),
        )
        .await
    {
        Ok(machine) => machine,
        Err(error) => {
            created.push((machine_path, format!("VM {name}")));
            return Err(failed(created, format!("the VM {name} was not created: {error}")).await);
        }
    };
    let vm_id = match machine.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => {
            created.push((machine_path, format!("VM {name}")));
            return Err(failed(
                created,
                format!("the VM {name} was created but names no id: {machine}"),
            )
            .await);
        }
    };
    Ok(Created {
        address: address_text,
        vm_id,
    })
}

/// Make `sources` the addresses host `name`'s own security group admits on
/// SSH.
pub async fn admit_host(
    client: &ArmClient,
    rg: &str,
    name: &str,
    sources: &[String],
) -> Result<(), String> {
    let rule = Inbound {
        name: SSH_RULE,
        port: SSH_PORT,
        sources,
        destination_groups: &[],
        priority: super::public::LOWEST_RULE_PRIORITY,
    };
    let path = format!(
        "/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/networkSecurityGroups/{}/securityRules/{SSH_RULE}?api-version={NETWORK_API_VERSION}",
        client.subscription(),
        group_name(name)
    );
    client
        .put_lro(
            &path,
            &json!({ "properties": inbound_rule_properties(&rule) }),
            &format!("admit the fleet's addresses to {name}"),
        )
        .await
        .map(drop)
        .map_err(|error| format!("the SSH rule of {name} was not rewritten: {error}"))
}

/// Delete host `name`: the VM (its OS disk goes with it), then its
/// interface, security group and public address, each confirmed gone.
/// Returns what was deleted, or every resource still standing.
pub async fn delete(client: &ArmClient, rg: &str, name: &str) -> Result<Vec<String>, Vec<String>> {
    let subscription = client.subscription();
    let interface_name = super::network::nic_name(name);
    let resources = [
        (vm_api_path(client, rg, name), format!("VM {name}")),
        (
            network_path(subscription, rg, "networkInterfaces", &interface_name),
            format!("NIC {interface_name}"),
        ),
        (
            network_path(subscription, rg, "networkSecurityGroups", &group_name(name)),
            format!("security group {}", group_name(name)),
        ),
        (
            network_path(subscription, rg, "publicIPAddresses", &address_name(name)),
            format!("public IP {}", address_name(name)),
        ),
    ];
    let mut deleted = Vec::new();
    let mut standing = Vec::new();
    for (path, description) in &resources {
        match discard(client, path, description).await {
            Ok(()) => deleted.push(description.clone()),
            Err(problem) => standing.push(problem),
        }
    }
    if standing.is_empty() {
        Ok(deleted)
    } else {
        Err(standing)
    }
}

/// Host `name`'s VM as ARM reads it with its instance view, or `None` when
/// it does not exist.
pub async fn read(client: &ArmClient, rg: &str, name: &str) -> Result<Option<Value>, String> {
    client
        .get_vm(rg, name)
        .await
        .map_err(|error| format!("the VM {name} could not be read: {error}"))
}
