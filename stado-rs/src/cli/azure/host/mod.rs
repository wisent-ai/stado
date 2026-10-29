//! `stado cloud host create|list|delete|reap --provider azure`: fleet hosts
//! Stado rents from a cloud, under the operator's credit floor, and returns
//! when they stand idle.
//!
//! A cloud host is an ordinary registry target with one more declaration,
//! `targets[].cloud`: the provider, the location, the machine size and its
//! priced hour, how long it may stand idle, and what it was created as.
//! Everything else is the fleet's usual machinery: its SSH key is minted as
//! `fleet key generate` mints one, it is registered by the verified
//! `fleet enroll` path, and `stado host cargo <host>` builds on it.
//!
//! Only the fleet's own hosts reach it. `cloud_hosts.admit_from` in the
//! registry names them; each create and reap asks each of them for its
//! public egress address (`<api.url>/api/egress`, answered by the product
//! site with the address the request came from), records the answers in
//! `cloud_hosts.egress`, and admits exactly those addresses to SSH.
//!
//! Create is held to the Azure credit gate
//! ([`crate::providers::azure::credit`]): one hour of the new machine and of
//! every cloud host already running is taken from the balance, and what is
//! left must stay above the floor the operator's ruling names. The projected
//! hourly burn is printed before anything is created.

mod create;
mod declaration;
mod delete;
mod egress;
mod list;
mod reap;

use clap::{Args, Subcommand};

use super::CloudProvider;
use crate::cli::CmdError;
use crate::config;
use crate::providers::azure::ArmClient;

pub(crate) use reap::reap_pass;

#[derive(Subcommand)]
pub enum CloudHostCommands {
    /// Rent a fleet host from the cloud: price it against the credit floor,
    /// create it reachable only from the fleet's own addresses, and enroll it.
    Create(CreateArgs),
    /// Every cloud host the registry declares, with its machine state, its
    /// priced hour and how long it has stood idle.
    List(ProviderArgs),
    /// Delete one cloud host on the cloud and take it out of the registry.
    Delete(DeleteArgs),
    /// Readmit the fleet's current addresses, and delete every cloud host
    /// idle longer than it declares; without --apply only reports.
    Reap(ReapArgs),
}

#[derive(Args)]
pub struct ProviderArgs {
    /// Cloud the hosts are rented from. No provider is assumed.
    #[arg(long, value_enum)]
    provider: CloudProvider,
    /// Print the answer as JSON instead of lines.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct CreateArgs {
    /// Registry name of the new host (a lowercase target identifier).
    name: String,
    /// Cloud the host is rented from. No provider is assumed.
    #[arg(long, value_enum)]
    provider: CloudProvider,
    /// Location (region) the host is created in; the deployment's vnet and
    /// subnet must exist there.
    #[arg(long)]
    location: String,
    /// Machine size, as the provider names it (e.g. Standard_D64as_v5).
    #[arg(long)]
    size: String,
    /// OS disk size in GiB.
    #[arg(long)]
    disk_gb: i64,
    /// Minutes the host may stand with no work before `reap` deletes it.
    #[arg(long)]
    idle_minutes: i64,
    /// Print the receipt as JSON instead of lines; progress goes to stderr.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Registry name of the cloud host.
    name: String,
    /// Cloud the host is rented from. No provider is assumed.
    #[arg(long, value_enum)]
    provider: CloudProvider,
    /// Print the receipt as JSON instead of lines.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReapArgs {
    /// Cloud the hosts are rented from. No provider is assumed.
    #[arg(long, value_enum)]
    provider: CloudProvider,
    /// Delete the idle hosts; without it the pass only reports them.
    #[arg(long)]
    apply: bool,
    /// Print the receipt as JSON instead of lines.
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(command: CloudHostCommands) -> Result<(), CmdError> {
    match command {
        CloudHostCommands::Create(args) => create::create(args).await,
        CloudHostCommands::List(args) => list::list(args).await,
        CloudHostCommands::Delete(args) => delete::delete(args).await,
        CloudHostCommands::Reap(args) => reap::reap(args).await,
    }
}

/// The Azure bindings a cloud host is addressed with, every one declared by
/// the deployment, and the ARM client that addresses them. A missing binding
/// is refused naming every one that is missing; Azure disabled in this
/// profile is refused as the profile says.
struct Azure {
    client: ArmClient,
    resource_group: &'static str,
    vnet: &'static str,
    subnet: &'static str,
    image_urn: &'static str,
    username: &'static str,
}

fn azure() -> Result<Azure, CmdError> {
    crate::providers::require_enabled(crate::capabilities::ProviderId::Azure.as_str())?;
    let missing: Vec<&str> = [
        ("azure.subscription_id", config::azure_subscription_id()),
        ("azure.resource_group", config::azure_resource_group()),
        ("azure.vnet", config::azure_vnet()),
        ("azure.subnet", config::azure_subnet()),
        ("azure.image_urn", config::azure_image_urn()),
        ("azure.vm_username", config::azure_vm_username()),
    ]
    .into_iter()
    .filter(|(_, value)| value.is_empty())
    .map(|(key, _)| key)
    .collect();
    if !missing.is_empty() {
        return Err(CmdError::declaration(format!(
            "a cloud host on Azure is addressed with bindings this deployment does not declare: {}; \
             `stado config set <key> <value>` declares each",
            missing.join(", ")
        )));
    }
    Ok(Azure {
        client: ArmClient::new(config::azure_subscription_id()),
        resource_group: config::azure_resource_group(),
        vnet: config::azure_vnet(),
        subnet: config::azure_subnet(),
        image_urn: config::azure_image_urn(),
        username: config::azure_vm_username(),
    })
}
