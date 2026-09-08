//! The workload identity each provider authenticates with, and the managed
//! identity an agent VM needs in order to exist at all.

use crate::config;
use crate::doctor::{provider_enabled, Check, Findings, Status, LOCAL_PROVIDER};
use crate::providers;

// ---------------------------------------------------------------------------
// 3. Provider auth
// ---------------------------------------------------------------------------

pub(in crate::doctor) const PROVIDERS_ID: &str = "providers";
pub(in crate::doctor) const PROVIDERS_TITLE: &str = "Provider auth";
pub(in crate::doctor) const PROVIDERS_REMEDY: &str =
    "run each cloud provider only behind its adapter's managed workload identity; static \
     provider keys, cloud CLI sessions, and provider credential items in control-plane or \
     agent grants are unsupported";

/// The cheapest authenticated call each provider offers: list what it is
/// already running. Reports the exact upstream error, because "the
/// credentials are missing" and "the subscription is disabled" read alike
/// from a boolean and need opposite fixes.
pub(in crate::doctor) async fn check_provider_auth() -> Check {
    let mut findings = Findings::default();
    for name in config::wc_providers() {
        if name == LOCAL_PROVIDER {
            findings.note(
                Status::Pass,
                format!("{name}: device-local, no cloud API to authenticate against"),
            );
            continue;
        }
        let provider = match providers::get_provider(name) {
            Ok(provider) => provider,
            Err(err) => {
                findings.note(
                    Status::Fail,
                    format!("{name}: cannot construct provider: {err}"),
                );
                findings.remedy(PROVIDERS_REMEDY);
                continue;
            }
        };
        match provider.list_running_instances().await {
            Ok(running) => {
                let total: i64 = running.values().sum();
                findings.note(
                    Status::Pass,
                    format!("{name}: authenticated, {total} instance(s) running"),
                );
            }
            Err(err) => {
                findings.note(Status::Fail, format!("{name}: {err}"));
                findings.remedy(PROVIDERS_REMEDY);
            }
        }
    }
    if findings.notes.is_empty() {
        findings.note(
            Status::Fail,
            "WC_PROVIDERS lists no provider to check".to_string(),
        );
        let choices =
            crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Compute)
                .collect::<Vec<_>>()
                .join(", ");
        findings.remedy(format!(
            "set WC_PROVIDERS to one or more comma-separated providers: {choices}"
        ));
    }
    findings.into_check(PROVIDERS_ID, PROVIDERS_TITLE, PROVIDERS_REMEDY)
}

// ---------------------------------------------------------------------------
// 7. VM identity
// ---------------------------------------------------------------------------

pub(in crate::doctor) const IDENTITY_ID: &str = "vm-identity";
pub(in crate::doctor) const IDENTITY_TITLE: &str = "Azure VM identity";
pub(in crate::doctor) const IDENTITY_REMEDY: &str =
    "export AZURE_VM_IDENTITY_ID=/subscriptions/<sub>/resourceGroups/<rg>/providers/\
     Microsoft.ManagedIdentity/userAssignedIdentities/<name> (config key \
     azure.vm_identity_id), and grant that identity read/write on the queue container";

/// Without a user-assigned identity on the VM, the on-VM half of the
/// [`crate::azure_token`] chain resolves nothing: an agent VM carries no
/// service-principal env vars and no `az` CLI, so IMDS is the only source
/// left and IMDS answers only for a VM that has an identity attached. The
/// agent can then neither read the queue nor self-delete, so it bills
/// until an operator happens to notice.
pub(in crate::doctor) fn check_vm_identity() -> Check {
    if !provider_enabled(crate::capabilities::ProviderId::Azure) {
        return Check::pass(
            IDENTITY_ID,
            IDENTITY_TITLE,
            "azure is not in WC_PROVIDERS; no agent VM needs a managed identity".to_string(),
            IDENTITY_REMEDY,
        );
    }
    let identity = config::azure_vm_identity_id();
    if identity.is_empty() {
        return Check::fail(
            IDENTITY_ID,
            IDENTITY_TITLE,
            "AZURE_VM_IDENTITY_ID is empty while azure is in WC_PROVIDERS; VM create emits no \
             identity block, so the agent's IMDS token chain resolves nothing and it can \
             neither claim jobs nor self-delete"
                .to_string(),
            IDENTITY_REMEDY,
        );
    }
    Check::pass(
        IDENTITY_ID,
        IDENTITY_TITLE,
        format!("agent VMs get user-assigned identity {identity}"),
        IDENTITY_REMEDY,
    )
}
