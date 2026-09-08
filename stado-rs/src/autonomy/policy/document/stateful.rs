//! Is this resource holding data someone would miss?
//!
//! Three independent reads, any one of which is enough: the resource type is
//! a storage type outright, an operator labelled it stateful, or the provider
//! evidence shows an attached data disk that outlives the instance. GCP,
//! Azure and AWS each describe that last one differently, so each is read on
//! its own terms and the three answers are folded together.

use crate::autonomy::model::ResourceRecord;

pub(super) fn is_stateful(resource: &ResourceRecord) -> bool {
    if matches!(
        resource.resource_type.as_str(),
        "database" | "cloud_sql" | "rds" | "managed_disk" | "persistent_disk" | "volume"
    ) {
        return true;
    }
    if resource.labels.iter().any(|(key, value)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "stateful" | "stado-stateful" | "stado.io/stateful"
        ) && matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "yes" | "stateful"
        )
    }) {
        return true;
    }
    let gcp_data_disk = resource
        .evidence
        .pointer("/item/disks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|disks| {
            disks.iter().any(|disk| {
                disk.get("boot").and_then(serde_json::Value::as_bool) == Some(false)
                    || disk.get("autoDelete").and_then(serde_json::Value::as_bool) == Some(false)
            })
        });
    let azure_data_disk = resource
        .evidence
        .pointer("/properties/storageProfile/dataDisks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|disks| !disks.is_empty());
    let aws_root = resource
        .evidence
        .get("root_device_name")
        .and_then(serde_json::Value::as_str);
    let aws_data_disk = resource
        .evidence
        .get("block_devices")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|devices| {
            devices.iter().any(|device| {
                device
                    .get("delete_on_termination")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                    || aws_root.is_some_and(|root| {
                        device
                            .get("device_name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|name| name != root)
                    })
            })
        });
    gcp_data_disk || azure_data_disk || aws_data_disk
}
