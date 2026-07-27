//! Azure network helpers — subnet/NSG ARM ID construction + NIC lifecycle.
//!
//! Port of `stado/providers/azure_helpers/network.py`.
//!
//! Naming convention: vnet `${AZURE_VNET}-{location}`, NSG `${AZURE_NSG}-
//! {location}`, subnet `${AZURE_SUBNET}` (subnet is a child of vnet so its
//! name is locally scoped to the already-region-specific vnet).

use serde_json::{json, Value};

use super::{ArmClient, AzureError, NETWORK_API_VERSION};

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[azure-net] {msg}");
}

/// Python `subnet_id`.
pub fn subnet_id(subscription: &str, rg: &str, vnet: &str, subnet: &str, location: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{rg}\
         /providers/Microsoft.Network/virtualNetworks/{vnet}-{location}\
         /subnets/{subnet}"
    )
}

/// Python `nsg_id` (empty NSG -> "").
pub fn nsg_id(subscription: &str, rg: &str, nsg: &str, location: &str) -> String {
    if nsg.is_empty() {
        return String::new();
    }
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{rg}\
         /providers/Microsoft.Network/networkSecurityGroups/{nsg}-{location}"
    )
}

/// NIC name for a VM (Python `f"{name}-nic"`).
pub fn nic_name(name: &str) -> String {
    format!("{name}-nic")
}

/// ARM resource path of the NIC (no api-version).
fn nic_path(subscription: &str, rg: &str, name: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{rg}\
         /providers/Microsoft.Network/networkInterfaces/{}",
        nic_name(name)
    )
}

/// Python `create_nic` body. Split out pure for tests.
pub fn nic_body(location: &str, subnet_id_str: &str, nsg_id_str: &str) -> Value {
    let mut properties = json!({
        "ipConfigurations": [{
            "name": "ipcfg",
            "properties": { "subnet": { "id": subnet_id_str } },
        }],
    });
    if !nsg_id_str.is_empty() {
        properties["networkSecurityGroup"] = json!({ "id": nsg_id_str });
    }
    json!({ "location": location, "properties": properties })
}

/// Python `create_nic`: PUT + wait for the LRO, returns the NIC ARM id.
pub async fn create_nic(
    client: &ArmClient,
    rg: &str,
    name: &str,
    location: &str,
    subnet_id_str: &str,
    nsg_id_str: &str,
) -> Result<String, AzureError> {
    let path = format!(
        "{}?api-version={NETWORK_API_VERSION}",
        nic_path(client.subscription(), rg, name)
    );
    let body = nic_body(location, subnet_id_str, nsg_id_str);
    let desc = format!("create NIC {}", nic_name(name));
    let nic = client.put_lro(&path, &body, &desc).await?;
    nic.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AzureError::Api(format!("Azure {desc} -> response has no id")))
}

/// Python `delete_nic`: best-effort — every failure is logged, never
/// propagated (Python swallows all exceptions here).
pub async fn delete_nic(client: &ArmClient, rg: &str, name: &str) {
    let path = format!(
        "{}?api-version={NETWORK_API_VERSION}",
        nic_path(client.subscription(), rg, name)
    );
    let desc = format!("delete NIC {}", nic_name(name));
    if let Err(err) = client.delete_allow_404(&path, &desc).await {
        log(&format!(
            "NIC delete failed for {}: {err:?}",
            nic_name(name)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_ids_match_python() {
        assert_eq!(
            subnet_id(
                "sub-1",
                "rg",
                "wisent-compute-vnet",
                "wisent-compute-subnet",
                "eastus"
            ),
            "/subscriptions/sub-1/resourceGroups/rg/providers/Microsoft.Network/\
             virtualNetworks/wisent-compute-vnet-eastus/subnets/wisent-compute-subnet"
        );
        assert_eq!(
            nsg_id("sub-1", "rg", "wisent-compute-nsg", "westus3"),
            "/subscriptions/sub-1/resourceGroups/rg/providers/Microsoft.Network/\
             networkSecurityGroups/wisent-compute-nsg-westus3"
        );
        assert_eq!(nsg_id("sub-1", "rg", "", "eastus"), "");
    }

    #[test]
    fn nic_body_shape() {
        let body = nic_body("eastus", "/subnet/id", "/nsg/id");
        assert_eq!(body["location"], json!("eastus"));
        assert_eq!(
            body["properties"]["ipConfigurations"][0]["properties"]["subnet"]["id"],
            json!("/subnet/id")
        );
        assert_eq!(
            body["properties"]["networkSecurityGroup"]["id"],
            json!("/nsg/id")
        );

        // Empty NSG -> no networkSecurityGroup key (Python omits it).
        let no_nsg = nic_body("eastus", "/subnet/id", "");
        assert!(no_nsg["properties"].get("networkSecurityGroup").is_none());
    }
}
