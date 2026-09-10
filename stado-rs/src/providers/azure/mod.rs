//! Azure provider: VM lifecycle over ARM REST, mirrors providers/gcp.
//!
//! Port of `stado/providers/azure.py`. Python uses azure-identity +
//! azure-mgmt-compute/azure-mgmt-network; this port talks to the ARM REST
//! API (`https://management.azure.com`) directly with reqwest — no Azure
//! SDK crates. NIC + VM create across AZURE_LOCATIONS, falling through
//! quota/capacity errors per region. Pre-provisioned vnet/NSG (named with
//! `-{location}` suffix, one per region in a shared RG) attach to the NIC;
//! the provider does not create networking. instance_ref is
//! `"name@location"`.
//!
//! Authentication is shared with the Azure Blob queue backend through
//! [`crate::remote::azure_token`]: an Azure managed identity is preferred, then the
//! `stado-azure` service-principal item is read from Skarbiec. This module
//! requests the ARM audience (`https://management.azure.com`).
//!
//! On an agent VM IMDS resolves only when the VM carries a managed identity.
//! Agent VMs are therefore created with the pre-provisioned user-assigned
//! identity named by
//! [`crate::config::azure_vm_identity_id`] (`AZURE_VM_IDENTITY_ID`), whose
//! resource id [`vm_body`] renders into the ARM `identity` block. The
//! operator grants that single identity, once:
//!
//! - `Storage Blob Data Contributor` on the queue storage account. That is
//!   a data-plane role; `Contributor` is management-plane only and does NOT
//!   authorize blob reads or writes.
//! - Permission to delete VMs in the resource group, so an idle agent can
//!   ARM-DELETE itself.
//!
//! Without it the agent can neither reach the blob queue (it never sees a
//! job, so the fleet is inert) nor self-delete (the VM bills until someone
//! notices). User-assigned rather than system-assigned is deliberate: a
//! system-assigned principal is minted per VM, so it would need its own
//! role assignment at create time and would leave orphans behind at
//! self-delete. Creating the identity and granting it those roles is an
//! operator provisioning step — this provider hands out no role
//! assignments, just as it creates no networking.
//!
//! Long-running operations are polled via the Azure-AsyncOperation header
//! (falling back to Location) until terminal — the equivalent of the Python
//! SDK's `op.result()`.
//!
//! Deviation: Python's `AzureProvider()` constructor eagerly raises
//! RuntimeError when AZURE_SUBSCRIPTION_ID is empty. Here
//! [`AzureProvider::from_env`] is lazy (same pattern as
//! [`super::gcp::GcpProvider`]): the check fires on the first API call so
//! `get_provider("azure")` stays a cheap, sync factory.

mod arm;
mod builders;
pub mod network;
mod provider;

pub use arm::{ArmClient, AzureError, ARM_API_BASE};
pub use builders::{parse_image_urn, power_state, vm_body, vm_is_alive};
pub use provider::AzureProvider;

pub(crate) use arm::{vm_path, COMPUTE_API_VERSION};

// `network` names this through `super::`, keeping its import line — and
// every other Azure caller's — unchanged by the split.
use arm::NETWORK_API_VERSION;
