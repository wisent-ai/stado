//! Azure compute settings.

use std::sync::LazyLock;

use crate::config::{resolve_compute_binding, resolve_compute_list_binding};

// Azure (parallel to GCP). All values resolved from env so the same
// wisent-compute install can target multiple subscriptions/resource groups
// without code changes. The provider does NOT create the vnet/subnet/NSG —
// it expects pre-provisioned infra named below.
static AZURE_SUBSCRIPTION_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "subscription-id",
        "",
    )
});
static AZURE_RESOURCE_GROUP: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "resource-group",
        "wisent-compute",
    )
});
static AZURE_LOCATIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    resolve_compute_list_binding(
        crate::capabilities::ProviderId::Azure,
        "locations",
        &["eastus", "westus3", "westus2", "northeurope"],
    )
});
static AZURE_VNET: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "vnet",
        "wisent-compute-vnet",
    )
});
static AZURE_SUBNET: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "subnet",
        "wisent-compute-subnet",
    )
});
static AZURE_NSG: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "nsg",
        "wisent-compute-nsg",
    )
});
static AZURE_IMAGE_URN: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "image-urn",
        "microsoft-dsvm:ubuntu-hpc:2204:latest",
    )
});
static AZURE_VM_USERNAME: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "vm-username",
        "wisent",
    )
});
static AZURE_SSH_PUBLIC_KEY: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Azure, "ssh-public-key", "")
});
static AZURE_VM_IDENTITY_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Azure, "vm-identity-id", "")
});

/// Azure subscription id (env `AZURE_SUBSCRIPTION_ID`).
pub fn azure_subscription_id() -> &'static str {
    AZURE_SUBSCRIPTION_ID.as_str()
}

/// Azure resource group (env `AZURE_RESOURCE_GROUP`).
pub fn azure_resource_group() -> &'static str {
    AZURE_RESOURCE_GROUP.as_str()
}

/// Azure dispatch locations (env `AZURE_LOCATIONS`, comma-separated).
pub fn azure_locations() -> &'static [String] {
    &AZURE_LOCATIONS
}

/// Pre-provisioned Azure vnet name (env `AZURE_VNET`).
pub fn azure_vnet() -> &'static str {
    AZURE_VNET.as_str()
}

/// Pre-provisioned Azure subnet name (env `AZURE_SUBNET`).
pub fn azure_subnet() -> &'static str {
    AZURE_SUBNET.as_str()
}

/// Pre-provisioned Azure NSG name (env `AZURE_NSG`).
pub fn azure_nsg() -> &'static str {
    AZURE_NSG.as_str()
}

/// Azure base image URN (env `AZURE_IMAGE_URN`,
/// publisher:offer:sku:version). microsoft-dsvm:ubuntu-hpc:2204:latest
/// ships with NVIDIA driver + CUDA preinstalled, matching
/// deeplearning-platform-release on GCP.
pub fn azure_image_urn() -> &'static str {
    AZURE_IMAGE_URN.as_str()
}

/// Azure cloud-init admin username (env `AZURE_VM_USERNAME`).
pub fn azure_vm_username() -> &'static str {
    AZURE_VM_USERNAME.as_str()
}

/// SSH public key for the cloud-init admin user (env
/// `AZURE_SSH_PUBLIC_KEY`). Required by Azure VM create even when SSH is
/// locked down via NSG; cloud-init will only accept the VM create call if
/// either ssh keys or password auth is configured.
pub fn azure_ssh_public_key() -> &'static str {
    AZURE_SSH_PUBLIC_KEY.as_str()
}

/// Resource id of the pre-provisioned user-assigned managed identity
/// attached to every agent VM (env `AZURE_VM_IDENTITY_ID`): a full ARM
/// path under
/// `.../providers/Microsoft.ManagedIdentity/userAssignedIdentities/`.
/// This is how the agent gets Azure credentials at all — on the VM the
/// token chain in [`crate::azure_token`] has no service-principal env
/// vars and no `az` CLI, so it falls through to IMDS, which answers only
/// for a VM that carries an identity. Empty (the default) emits no
/// identity block at VM create, leaving the agent unable to reach the
/// blob queue or to self-delete.
pub fn azure_vm_identity_id() -> &'static str {
    AZURE_VM_IDENTITY_ID.as_str()
}
