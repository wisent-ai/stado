//! The ARM request bodies the edge's resources are created from, and the
//! path they are sent to.

use serde_json::{json, Value};

use super::super::NETWORK_API_VERSION;

pub(super) fn public_ip_body(region: &str, name: &str) -> Value {
    json!({
        "location": region,
        // Standard SKU, and therefore static: a dynamic address is reassigned
        // when the VM stops, and every product hostname's A record points at
        // this one.
        "sku": { "name": "Standard" },
        "properties": {
            "publicIPAllocationMethod": "Static",
            "publicIPAddressVersion": "IPv4",
        },
        "tags": { "wisent_managed": "true", "wisent_role": "web-edge", "wisent_edge": name },
    })
}

/// The edge's own security group.
///
/// Deliberately not the pre-provisioned group the agent VMs share: opening 80
/// and 443 there would open them on every agent VM in the region, and the two
/// hosts have nothing in common but a subnet. Only the two ports the proxy
/// serves are opened. Nothing opens 22, because the host channel reaches the
/// edge over the tailnet like every other fleet host, and Azure's default
/// rules already allow the outbound traffic tailscaled dials out with.
pub(super) fn security_group_body(region: &str, name: &str) -> Value {
    let rule = |rule_name: &str, port: &str, priority: i64| {
        json!({
            "name": rule_name,
            "properties": {
                "protocol": "Tcp",
                "sourcePortRange": "*",
                "destinationPortRange": port,
                "sourceAddressPrefix": "Internet",
                "destinationAddressPrefix": "*",
                "access": "Allow",
                "priority": priority,
                "direction": "Inbound",
            },
        })
    };
    json!({
        "location": region,
        "properties": {
            "securityRules": [
                // 80 is not optional: Let's Encrypt's HTTP-01 challenge
                // arrives there, and so does every redirect to HTTPS.
                rule("allow-http", "80", 300),
                rule("allow-https", "443", 310),
            ],
        },
        "tags": { "wisent_managed": "true", "wisent_role": "web-edge", "wisent_edge": name },
    })
}

pub(super) fn interface_body(
    region: &str,
    subnet: &str,
    security_group: &str,
    public_ip: &str,
) -> Value {
    json!({
        "location": region,
        "properties": {
            "networkSecurityGroup": { "id": security_group },
            "ipConfigurations": [{
                "name": "ipcfg",
                "properties": {
                    "subnet": { "id": subnet },
                    "publicIPAddress": { "id": public_ip },
                    "privateIPAllocationMethod": "Dynamic",
                },
            }],
        },
    })
}

pub(super) fn network_path(
    subscription: &str,
    resource_group: &str,
    kind: &str,
    name: &str,
) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{resource_group}\
         /providers/Microsoft.Network/{kind}/{name}\
         ?api-version={NETWORK_API_VERSION}"
    )
}
