//! `stado instances` — read-only cross-provider agent-VM inventory.
//!
//! NO Python original: the Python CLI never exposed the running cloud fleet
//! at all. Both providers that can enumerate their VMs
//! (`providers/gcp/mod.rs::GcpProvider::list_running_instance_refs_with_age`
//! and `providers/azure/mod.rs::AzureProvider::list_running_instance_refs_with_age`)
//! were reachable only from `monitor/monitor.rs::reap_dead_agents`, which
//! runs inside the coordinator tick and prints nothing an operator can read.
//! A VM whose agent died therefore billed silently until someone opened the
//! cloud console — the July host incident and the GCP-billing outage were
//! both found that way.
//!
//! Uniform provider access: enumeration rides the existing optional trait
//! method `providers/mod.rs::Provider::list_running_instance_refs_with_age`
//! rather than provider-specific match arms here.
//!   * gcp — overrides it (aggregated list, non-TERMINATED `<prefix>-agent-*`).
//!   * azure — overrides it, forwarding to the inherent method.
//!   * box — inherits the base default; a box is rented per job through
//!     `queue/leases.rs`, there is no standing VM fleet to sweep.
//!   * aws — inherits the base default (empty). `providers/aws.rs::Ec2Api`
//!     exposes only `running_instance_types`, no per-instance enumeration,
//!     so an AWS row cannot be produced without widening that trait. AWS
//!     therefore reports an empty fleet here, and `instances list` names
//!     every provider that reported nothing rather than letting an
//!     unenumerable cloud render as "no orphans".
//!   * vast — not a `Provider` at all (wisent-compute is the marketplace
//!     HOST there, see `providers/vast.rs`), so it has no fleet to list.
//!
//! Ownership cross-check: a VM is an ORPHAN when nothing in the store still
//! claims it — no `running/` job document carries it as `instance_ref`, and
//! no un-released `provider-leases/` blob names it as its
//! `provider_resource_id`. That is the column the operator is looking for;
//! everything else on the row exists to justify it.
//!
//! One component per seam: `holders` is the store-side ownership index that
//! decides whether a VM is still claimed, `fleet` is the cross-provider
//! enumeration plus the read-only audit projection the resource commands
//! consume, `list` is the `instances list` body, and `output` holds the
//! formatting, the error printing and the exit status those two print
//! through. The command surface — the clap types, the subcommand dispatch
//! and the provider selection — stays here.

mod fleet;
mod holders;
mod list;
mod output;

pub(crate) use fleet::{audit_inventory, AuditInstanceRow};

use clap::{Args, Subcommand};

use crate::config;

use super::CmdError;

use list::list;

#[derive(Subcommand)]
pub enum InstancesCommands {
    /// List every live agent VM across the configured providers, flagging
    /// the ones no queue job or lease still references.
    List(InstancesListArgs),
}

#[derive(Args, Debug)]
pub struct InstancesListArgs {
    /// Single provider to inspect; default is every entry in WC_PROVIDERS.
    #[arg(long)]
    provider: Option<String>,
    /// Emit machine-readable JSON instead of the table.
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(cmd: InstancesCommands) -> Result<(), CmdError> {
    match cmd {
        InstancesCommands::List(args) => list(&args).await,
    }
}

/// Providers that expose an inventory adapter: the selected provider or every
/// configured provider, in catalog order.
fn fleet_providers(selected: Option<&str>) -> Result<Vec<String>, CmdError> {
    let configured = match selected {
        Some(name) => vec![name.trim().to_string()],
        None => config::wc_providers().to_vec(),
    };
    let enumerable =
        crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Inventory);
    let fleet = configured
        .into_iter()
        .filter_map(|name| {
            crate::capabilities::provider(&name)
                .filter(|provider| enumerable.contains(provider))
                .map(|provider| provider.as_str().to_string())
        })
        .collect::<Vec<_>>();
    if fleet.is_empty() {
        let choices = enumerable
            .into_iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CmdError::click(format!(
            "no provider with an agent-VM inventory selected; available inventory providers: {choices}"
        )));
    }
    Ok(fleet)
}
