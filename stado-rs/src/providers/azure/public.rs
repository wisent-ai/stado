//! The ARM resources a VM that the Internet reaches is made of: its static
//! public address, its own security group, its application security group,
//! and the interface that binds them; plus the delete that is only done when
//! Azure answers 404 afterwards, and the rollback built from it.
//!
//! The web edge and the fleet's cloud hosts are both such VMs, so both build
//! their requests here. Every resource carries [`MANAGED_TAG`], the role it
//! plays (`wisent_role`) and the host it belongs to (`wisent_host`), so an
//! inventory reads ownership from the tags and nothing else.

use serde_json::{json, Map, Value};

use super::{ArmClient, MANAGED_TAG, MANAGED_TAG_VALUE, NETWORK_API_VERSION};

/// The lowest priority Azure accepts for a security rule; rules are
/// evaluated from it upwards
/// (<https://learn.microsoft.com/en-us/azure/virtual-network/network-security-groups-overview#security-rules>).
pub const LOWEST_RULE_PRIORITY: i64 = 100;
/// The highest priority Azure accepts for a security rule (same page).
pub const HIGHEST_RULE_PRIORITY: i64 = 4096;

/// The tags every resource of one public VM carries.
pub fn tags(role: &str, host: &str) -> Value {
    json!({ (MANAGED_TAG): MANAGED_TAG_VALUE, "wisent_role": role, "wisent_host": host })
}

/// One inbound TCP allowance of a security rule: who may connect, to which
/// port, and where the rule sits in Azure's evaluation order.
pub struct Inbound<'a> {
    pub name: &'a str,
    pub port: &'a str,
    /// Address prefixes the rule admits: `Internet`, or the exact addresses
    /// of the hosts it is for.
    pub sources: &'a [String],
    /// Application security groups the rule admits to; empty admits to every
    /// interface the group is bound to.
    pub destination_groups: &'a [String],
    pub priority: i64,
}

/// The ARM `properties` of one inbound rule.
pub fn inbound_rule_properties(rule: &Inbound<'_>) -> Value {
    let mut properties = json!({
        "protocol": "Tcp",
        "sourcePortRange": "*",
        "destinationPortRange": rule.port,
        "access": "Allow",
        "priority": rule.priority,
        "direction": "Inbound",
    });
    match rule.sources {
        [single] => properties["sourceAddressPrefix"] = json!(single),
        many => properties["sourceAddressPrefixes"] = json!(many),
    }
    if rule.destination_groups.is_empty() {
        properties["destinationAddressPrefix"] = json!("*");
    } else {
        properties["destinationApplicationSecurityGroups"] = Value::Array(
            rule.destination_groups
                .iter()
                .map(|id| json!({ "id": id }))
                .collect(),
        );
    }
    properties
}

/// A static Standard public IPv4 address. Standard is the SKU, and it is
/// static: a dynamic address is reassigned when the VM stops, and whatever
/// points at the host points at this one.
pub fn public_ip_body(region: &str, tags: &Value) -> Value {
    json!({
        "location": region,
        "sku": { "name": "Standard" },
        "properties": {
            "publicIPAllocationMethod": "Static",
            "publicIPAddressVersion": "IPv4",
        },
        "tags": tags,
    })
}

/// A VM's own security group, opening exactly `rules`.
pub fn security_group_body(region: &str, tags: &Value, rules: &[Inbound<'_>]) -> Value {
    json!({
        "location": region,
        "properties": {
            "securityRules": rules
                .iter()
                .map(|rule| json!({ "name": rule.name, "properties": inbound_rule_properties(rule) }))
                .collect::<Vec<_>>(),
        },
        "tags": tags,
    })
}

/// An application security group: the name a subnet's rule admits an
/// interface by, so the rule opens the interfaces bound to it and no other.
pub fn application_security_group_body(region: &str, tags: &Value) -> Value {
    json!({ "location": region, "properties": {}, "tags": tags })
}

/// The interface binding the subnet, the security group, the public address
/// and the application security groups.
pub fn interface_body(
    region: &str,
    subnet: &str,
    security_group: &str,
    public_ip: &str,
    application_groups: &[String],
    tags: &Value,
) -> Value {
    let mut configuration = Map::new();
    configuration.insert("subnet".into(), json!({ "id": subnet }));
    configuration.insert("publicIPAddress".into(), json!({ "id": public_ip }));
    configuration.insert("privateIPAllocationMethod".into(), json!("Dynamic"));
    if !application_groups.is_empty() {
        configuration.insert(
            "applicationSecurityGroups".into(),
            Value::Array(
                application_groups
                    .iter()
                    .map(|id| json!({ "id": id }))
                    .collect(),
            ),
        );
    }
    json!({
        "location": region,
        "properties": {
            "networkSecurityGroup": { "id": security_group },
            "ipConfigurations": [{ "name": "ipcfg", "properties": Value::Object(configuration) }],
        },
        "tags": tags,
    })
}

/// The ARM path of one Network resource, with its API version.
pub fn network_path(subscription: &str, resource_group: &str, kind: &str, name: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{resource_group}\
         /providers/Microsoft.Network/{kind}/{name}\
         ?api-version={NETWORK_API_VERSION}"
    )
}

/// Delete one ARM resource, follow Azure's own long-running operation to its
/// end, and confirm reading it answers 404.
///
/// A public address cannot be removed while the interface still references
/// it, so a rollback that fired three deletes at once would leave the address
/// behind — billed, unattached, and belonging to nothing. `get_allow_404`
/// returning `None` is the only evidence that the resource is actually gone;
/// a resource Azure still returns after its delete operation finished is an
/// error naming it.
pub async fn discard(client: &ArmClient, path: &str, description: &str) -> Result<(), String> {
    client
        .delete_lro_allow_404(path, description)
        .await
        .map_err(|error| error.to_string())?;
    match client.get_allow_404(path, description).await {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(format!(
            "Azure finished deleting {description} ({path}) and still returns it"
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// Remove, in reverse creation order, what a failed provision created.
///
/// Every failure is collected rather than propagated: the caller is already
/// returning Azure's own refusal, and what an operator needs added to it is
/// the list of resources that are still there.
pub async fn unwind(client: &ArmClient, created: &[(String, String)]) -> Vec<String> {
    let mut leftovers = Vec::new();
    for (path, description) in created.iter().rev() {
        if let Err(error) = discard(client, path, description).await {
            leftovers.push(error);
        }
    }
    leftovers
}

/// The first priority at or above [`LOWEST_RULE_PRIORITY`] no rule of
/// `group` (an ARM security group document) holds, except `keep`, the rule
/// about to be written, whose own priority is reused.
pub fn free_priority(group: &Value, keep: &str) -> Result<i64, String> {
    let rules = group
        .pointer("/properties/securityRules")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "Azure answered the security group {} without its securityRules: {group}",
                group["id"]
            )
        })?;
    if let Some(own) = rules
        .iter()
        .find(|rule| rule["name"].as_str() == Some(keep))
        .and_then(|rule| rule.pointer("/properties/priority"))
        .and_then(Value::as_i64)
    {
        return Ok(own);
    }
    let taken: Vec<i64> = rules
        .iter()
        .filter_map(|rule| rule.pointer("/properties/priority").and_then(Value::as_i64))
        .collect();
    (LOWEST_RULE_PRIORITY..=HIGHEST_RULE_PRIORITY)
        .find(|priority| !taken.contains(priority))
        .ok_or_else(|| {
            format!(
                "every priority from {LOWEST_RULE_PRIORITY} to {HIGHEST_RULE_PRIORITY} is held by a rule of {}",
                group["id"]
            )
        })
}
