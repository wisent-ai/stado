//! Azure VM self-awareness helpers for the agent.
//!
//! Azure counterpart of [`super::gcp_self`]. It has no Python source:
//! `stado/providers/local/gcp_self.py` only ever knew about GCE, so an
//! `--idle-shutdown` agent on an Azure VM exited its process and left the
//! VM running — and billing — until someone noticed. Everything here is a
//! Rust-side addition shaped to match the GCE module it mirrors.
//!
//! Only the agent's idle-shutdown / drift branches reach this, through
//! [`super::self_terminate`]; the workstation and Vast.ai modes never do.
//!
//! Unlike GCE (`gcloud compute instances delete`), the delete goes
//! straight at the ARM REST API via [`crate::providers::azure`]'s
//! [`ArmClient`]: the agent image carries no `az` CLI, and the shared
//! token chain in [`crate::azure_token`] already knows how to get an ARM
//! bearer token from the VM's managed identity.
//!
//! PERMISSIONS: the delete only lands if the identity the agent
//! authenticates as may run `Microsoft.Compute/virtualMachines/delete` on
//! its own resource group. The desktop provisioner already grants the
//! control-plane managed identity `Contributor` at resource-group scope
//! (`desktop/StadoDesktop/Sources/Stado/BackendProvisioner.swift`,
//! `assignRoles`), which covers it; a VM that authenticates as any other
//! principal needs the equivalent grant. Without it ARM answers Forbidden
//! and self-termination degrades to a logged no-op — the VM stays up,
//! exactly as it does today.

use std::time::Duration;

use serde_json::Value;

use crate::providers::azure::{vm_path, ArmClient, COMPUTE_API_VERSION};

/// Azure Instance Metadata Service instance document — the counterpart of
/// [`super::gcp_self::METADATA_BASE`].
pub const IMDS_INSTANCE_URL: &str = "http://169.254.169.254/metadata/instance";

/// Pinned IMDS API version. IMDS versions the whole service, not the
/// endpoint, so this is the same value the managed-identity token request
/// in [`crate::azure_token`] pins; the three `compute` fields read below
/// have been present since well before it.
const IMDS_API_VERSION: &str = crate::azure_token::IMDS_API_VERSION;

/// The VM this process is running on, as IMDS reports it.
///
/// Taken from metadata rather than `AZURE_SUBSCRIPTION_ID` / the config
/// file on purpose: the VM names itself, so a self-terminate can only
/// ever target the box it runs on, and an agent whose env was never given
/// Azure coordinates still shuts itself down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfVm {
    /// `compute.subscriptionId`.
    pub subscription: String,
    /// `compute.resourceGroupName`.
    pub resource_group: String,
    /// `compute.name` — the ARM VM resource name, not the OS hostname
    /// (Azure truncates a Linux `computerName`; this is not truncated).
    pub name: String,
}

/// Raw IMDS instance-metadata document for this VM.
pub async fn fetch_instance_metadata() -> Result<Value, reqwest::Error> {
    fetch_instance_metadata_at(IMDS_INSTANCE_URL, super::gcp_self::metadata_timeout()).await
}

/// [`fetch_instance_metadata`] against an explicit URL (a loopback
/// playback server in tests). The `Metadata: true` header is mandatory —
/// IMDS rejects requests without it, which is also what stops a stray
/// proxy from answering in its place.
///
/// `timeout` must stay short: off Azure this link-local address usually
/// black-holes packets instead of refusing them, so an unbounded probe
/// would stall every workstation shutdown. Production callers pass the
/// same ceiling the GCE probe uses.
pub async fn fetch_instance_metadata_at(
    url: &str,
    timeout: Duration,
) -> Result<Value, reqwest::Error> {
    reqwest::Client::new()
        .get(url)
        .header("Metadata", "true")
        .query(&[("api-version", IMDS_API_VERSION)])
        .timeout(timeout)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

/// True iff this process is running on an Azure VM (IMDS answers).
/// Counterpart of [`super::gcp_self::on_gcp`].
pub async fn on_azure() -> bool {
    fetch_instance_metadata().await.is_ok()
}

/// Pure: the VM's own ARM coordinates out of an IMDS document.
///
/// `None` when any of the three is missing or blank — a half-known
/// identity is never enough to issue a DELETE.
pub fn self_vm(metadata: &Value) -> Option<SelfVm> {
    let compute = metadata.get("compute")?;
    // Owned strings, not borrows of `metadata`: the caller keeps the
    // identity across the ARM call that outlives the IMDS document.
    let field = |key: &str| -> Option<String> {
        compute
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Some(SelfVm {
        subscription: field("subscriptionId")?,
        resource_group: field("resourceGroupName")?,
        name: field("name")?,
    })
}

/// If on Azure, delete this VM through ARM. Best-effort; failure is
/// non-fatal and only logged. Counterpart of
/// [`super::gcp_self::self_terminate`].
///
/// No-op outside Azure, so a misconfigured idle-shutdown on the
/// workstation cannot delete anything: an unreachable IMDS fails closed.
/// The single IMDS request does both jobs — it is the [`on_azure`] probe
/// *and* the source of the VM's identity, so the shutdown path pays one
/// round trip, not two.
///
/// The delete is fired and not awaited to completion: ARM answers
/// Accepted with a long-running-operation header, and this process is
/// about to exit anyway — the same reason [`ArmClient::delete_allow_404`]
/// does not poll. A NotFound means someone already deleted us, which is
/// the desired terminal state.
pub async fn self_terminate(log_fn: &mut dyn FnMut(&str)) {
    let Ok(metadata) = fetch_instance_metadata().await else {
        return;
    };
    let Some(vm) = self_vm(&metadata) else {
        log_fn(
            "Azure self-terminate failed: IMDS instance metadata has no \
             compute.subscriptionId / resourceGroupName / name",
        );
        return;
    };
    log_fn(&format!(
        "Azure self-terminate: delete virtualMachines/{} in resource group {}",
        vm.name, vm.resource_group
    ));
    let path = format!(
        "{}?api-version={COMPUTE_API_VERSION}",
        vm_path(&vm.subscription, &vm.resource_group, &vm.name)
    );
    match ArmClient::new(&vm.subscription)
        .delete_allow_404(&path, &format!("delete VM {}", vm.name))
        .await
    {
        Ok(true) => log_fn("Azure self-terminate: delete accepted"),
        Ok(false) => log_fn("Azure self-terminate: VM already gone"),
        Err(err) => log_fn(&format!("Azure self-terminate failed: {err}")),
    }
}
