//! Pure request builders and failure classification for the Azure
//! provider: image-URN parsing, the VM body, the protected agent-grant
//! extension, and the power/liveness predicates read off an ARM VM.

use base64::Engine as _;
use serde_json::{json, Value};

use super::arm::vm_path;

const VM_EXTENSION_API_VERSION: &str = "2022-11-01";
const AGENT_GRANT_EXTENSION_NAME: &str = "stado-agent-grant";

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
            "wisent_managed": "true",
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

/// Python `instance_exists` mapping (provisioning_state already
/// lowercased here; power_state is the raw string after `PowerState/`).
pub fn vm_is_alive(provisioning_state: Option<&str>, power_state: Option<&str>) -> bool {
    // provisioningState == "Succeeded" + power_state in (running,
    // starting) is the closest analogue to GCE
    // RUNNING/STAGING/PROVISIONING. Azure also has "Updating", which we
    // treat as alive — a VM mid-update is still consuming GPU quota and
    // shouldn't be requeued.
    let prov = provisioning_state.unwrap_or("").to_lowercase();
    if matches!(prov.as_str(), "creating" | "updating" | "succeeded") {
        if let Some(state) = power_state {
            return matches!(state, "running" | "starting");
        }
        // Mid-create: no PowerState yet — treat as alive.
        return matches!(prov.as_str(), "creating" | "updating");
    }
    false
}
