//! Pure request builders and failure classification for the Azure
//! provider: image-URN parsing, the VM body, the protected agent-grant
//! extension, and the power/liveness predicates read off an ARM VM.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use super::arm::vm_path;
use crate::providers::{Lifecycle, LifecyclePhase};

const VM_EXTENSION_API_VERSION: &str = "2022-11-01";
const AGENT_GRANT_EXTENSION_NAME: &str = "stado-agent-grant";
/// Tag Stado writes on every resource it creates; its value is [`MANAGED_TAG_VALUE`].
/// Inventory reads ownership from this tag, never from free text.
pub const MANAGED_TAG: &str = "wisent_managed";
pub const MANAGED_TAG_VALUE: &str = "true";

// --- Pure builders + classification (split out for tests) ---

/// Python `_parse_image_urn`. Err is the Python ValueError.
pub fn parse_image_urn(urn: &str) -> Result<Value, String> {
    let parts: Vec<&str> = urn.split(':').collect();
    if parts.len() != 4 {
        return Err(format!(
            "AZURE_IMAGE_URN must be 'publisher:offer:sku:version', got {urn:?}"
        ));
    }
    Ok(json!({
        "publisher": parts[0],
        "offer": parts[1],
        "sku": parts[2],
        "version": parts[3],
    }))
}

/// Python's VM body from `create_instance`. Split out pure for tests.
/// Err is the Python ValueError from `_parse_image_urn`.
///
/// Deviation from Python: a non-empty `identity_id` also renders the ARM
/// `identity` block, which is what gives the VM a token source (IMDS) for
/// the blob queue and for deleting itself. Empty renders the Python body
/// unchanged.
#[allow(clippy::too_many_arguments)]
pub fn vm_body(
    name: &str,
    location: &str,
    machine_type: &str,
    boot_disk_gb: i64,
    image_urn: &str,
    username: &str,
    ssh_public_key: &str,
    startup_script: &str,
    nic_id: &str,
    identity_id: &str,
    preemptible: bool,
) -> Result<Value, String> {
    let image_reference = parse_image_urn(image_urn)?;
    let ssh = if ssh_public_key.is_empty() {
        json!({})
    } else {
        json!({
            "publicKeys": [{
                "path": format!("/home/{username}/.ssh/authorized_keys"),
                "keyData": ssh_public_key,
            }],
        })
    };
    let mut properties = json!({
        "hardwareProfile": { "vmSize": machine_type },
        "storageProfile": {
            "imageReference": image_reference,
            "osDisk": {
                "createOption": "FromImage",
                "diskSizeGB": boot_disk_gb,
                "managedDisk": { "storageAccountType": "Premium_LRS" },
                "deleteOption": "Delete",
            },
        },
        "osProfile": {
            // Azure caps Linux hostname at 15.
            "computerName": name.chars().take(15).collect::<String>(),
            "adminUsername": username,
            "customData": base64::engine::general_purpose::STANDARD
                .encode(startup_script.as_bytes()),
            "linuxConfiguration": {
                "disablePasswordAuthentication": true,
                "ssh": ssh,
            },
        },
        "networkProfile": {
            "networkInterfaces": [{
                "id": nic_id,
                "properties": { "primary": true, "deleteOption": "Delete" },
            }],
        },
    });
    if preemptible {
        // Azure Spot: priority="Spot", eviction_policy="Delete" so a
        // preempted VM is fully removed (matches GCP's
        // instance_termination_action="DELETE"). billing_profile
        // max_price=-1 means "pay up to on-demand list price", i.e. take
        // whatever Spot capacity is available without an explicit cap.
        // The scheduler enforces cost via max_cost_per_hour_usd
        // separately.
        properties["priority"] = json!("Spot");
        properties["evictionPolicy"] = json!("Delete");
        properties["billingProfile"] = json!({ "maxPrice":
            -1.0 });
    }
    let mut body = json!({
        "location": location,
        "tags": {
            (MANAGED_TAG): MANAGED_TAG_VALUE,
            "wisent_created": chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, false),
        },
        "properties": properties,
    });
    if !identity_id.is_empty() {
        // ARM hangs `identity` off the resource root, beside `location` and
        // `properties`, not inside them. userAssignedIdentities is a map
        // keyed by the identity's resource id whose value ARM fills in with
        // principalId/clientId, so we send an empty object. Skipped when
        // unconfigured, keeping the rendered body byte-identical to the
        // Python original.
        body["identity"] = json!({
            "type": "UserAssigned",
            "userAssignedIdentities": { identity_id: {} },
        });
    }
    Ok(body)
}

pub(super) fn vm_extension_path(subscription: &str, resource_group: &str, vm_name: &str) -> String {
    format!(
        "{}/extensions/{AGENT_GRANT_EXTENSION_NAME}?api-version={VM_EXTENSION_API_VERSION}",
        vm_path(subscription, resource_group, vm_name)
    )
}

/// Script carried only in Azure `protectedSettings`. It writes the opaque
/// grant atomically into `/run` without placing it in argv/stdout, truncates
/// its own handler copy, and leaves the tmpfs file for the Rust client's
/// first-read cache to erase.
fn protected_agent_grant_script(agent_grant: &str) -> String {
    let encoded_grant = base64::engine::general_purpose::STANDARD.encode(agent_grant.as_bytes());
    format!(
        r#"#!/bin/sh
set -eu
set +x
umask 077
grant_dir=/run/stado-agent-credentials
grant_file=$grant_dir/skarbiec-token
grant_tmp=
grant_b64='{encoded_grant}'
cleanup() {{
    grant_b64=
    if [ -n "${{grant_tmp:-}}" ]; then
        : > "$grant_tmp" 2>/dev/null || true
        rm -f "$grant_tmp"
    fi
    if [ -f "$0" ]; then
        : > "$0" 2>/dev/null || true
        rm -f "$0"
    fi
}}
trap cleanup EXIT HUP INT TERM
install -d -m 0700 "$grant_dir"
grant_tmp="$(mktemp "$grant_dir/.skarbiec-token.XXXXXX")"
printf '%s' "$grant_b64" | base64 -d > "$grant_tmp"
grant_b64=
chmod 0600 "$grant_tmp"
mv -f "$grant_tmp" "$grant_file"
grant_tmp=
"#
    )
}

pub(super) fn agent_grant_extension_body(location: &str, agent_grant: &str) -> Value {
    let protected_script = base64::engine::general_purpose::STANDARD
        .encode(protected_agent_grant_script(agent_grant).as_bytes());
    json!({
        "location": location,
        "properties": {
            "publisher": "Microsoft.Azure.Extensions",
            "type": "CustomScript",
            "typeHandlerVersion": "2.1",
            "autoUpgradeMinorVersion": true,
            "enableAutomaticUpgrade": true,
            "settings": {},
            "protectedSettings": {
                "script": protected_script,
            },
        },
    })
}

/// Python NIC-create failure classification: QuotaExceeded /
/// OperationNotAllowed mark the location as skipped.
pub(super) fn nic_skip_error(msg: &str) -> bool {
    msg.contains("QuotaExceeded") || msg.contains("OperationNotAllowed")
}

/// Python VM-create failure classification adds SkuNotAvailable.
pub(super) fn vm_skip_error(msg: &str) -> bool {
    nic_skip_error(msg) || msg.contains("SkuNotAvailable")
}

/// First `PowerState/...` code of the instanceView ("running",
/// "deallocated", ...), None when absent.
pub fn power_state(vm: &Value) -> Option<String> {
    let statuses = vm
        .get("properties")?
        .get("instanceView")?
        .get("statuses")?
        .as_array()?;
    for status in statuses {
        if let Some(code) = status.get("code").and_then(Value::as_str) {
            if let Some(state) = code.strip_prefix("PowerState/") {
                return Some(state.to_string());
            }
        }
    }
    None
}

/// A VM's `properties.provisioningState`, as Azure Resource Manager
/// documents it. A value outside the list parses to nothing, and an
/// unreadable state is not alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningState {
    Creating,
    Updating,
    Succeeded,
    Failed,
    Deleting,
    Canceled,
    Migrating,
}

/// A VM's `PowerState/<code>` from its instance view, as Azure documents
/// the codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Deallocating,
    Deallocated,
    Unknown,
}

impl PowerState {
    /// What the power code means for the VM's life.
    fn phase(self) -> LifecyclePhase {
        match self {
            Self::Starting | Self::Running => LifecyclePhase::Running,
            Self::Stopping | Self::Stopped | Self::Deallocating | Self::Deallocated => {
                LifecyclePhase::Stopped
            }
            Self::Unknown => LifecyclePhase::Unknown,
        }
    }
}

/// The VM's lifecycle from its power code, `None` when it has none yet.
pub fn lifecycle(vm: &Value) -> Option<Lifecycle> {
    let word = power_state(vm)?;
    let phase = PowerState::deserialize(Value::String(word.clone()))
        .map_or(LifecyclePhase::Unknown, PowerState::phase);
    Some(Lifecycle { word, phase })
}

/// Python `instance_exists` mapping: a VM being created or updated, or
/// created, whose power is coming up or on, is the closest analogue to GCE
/// RUNNING/STAGING/PROVISIONING. A VM mid-update still consumes GPU quota
/// and is not requeued; mid-create it has no power state yet and is alive.
pub fn vm_is_alive(vm: &Value) -> bool {
    let provisioning = ProvisioningState::deserialize(&vm["properties"]["provisioningState"]).ok();
    let starting = matches!(
        provisioning,
        Some(ProvisioningState::Creating | ProvisioningState::Updating)
    );
    // No power code yet is mid-create; a code outside the documented list
    // is not a power that is coming up or on.
    match power_state(vm) {
        None => starting,
        Some(code) => {
            matches!(
                PowerState::deserialize(Value::String(code)),
                Ok(PowerState::Running | PowerState::Starting)
            ) && (starting || provisioning == Some(ProvisioningState::Succeeded))
        }
    }
}
