//! GPU and cloud compute vendors as Stado compute providers.
//!
//! Every vendor in [`GpuCloudVendor`] is an ordinary entry of `WC_PROVIDERS`:
//! the coordinator dispatches agent machines on it, reaps dead ones, counts
//! them against `config/quotas.json`, and `stado instances list` and
//! `stado doctor` read them. The shared lifecycle — names, the guest hostname
//! the reaper matches, idempotent release, the accelerator census — is
//! [`GpuCloudProvider`]; each vendor module only translates its own API
//! ([`GpuCloudApi`]) and declares its settings and its offer ladder
//! ([`VendorProfile`]).
//!
//! A vendor's credential is the Skarbiec item tagged
//! `stado:role:cloud-<provider>`; its settings live under `<provider>.*` in
//! the selected profile. Neither has a fallback.

pub mod access;
pub mod api;
pub mod http;
mod provider;
pub mod vendors;

pub use api::{GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState};
pub use provider::GpuCloudProvider;

use crate::capabilities::{ConfigField, GpuCloudVendor};

/// How the agent on a rented machine learns the name it publishes capacity
/// under (`<provider>-<name>`), which is the name the reaper looks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestIdentity {
    /// A virtual or bare-metal machine that runs the startup script as root:
    /// the script's first line sets the kernel hostname to the instance name.
    InstanceName,
    /// A container named by Stado, whose hostname the guest cannot set: the
    /// script exports `STADO_WORKER_NAME` as the instance name.
    ContainerName,
    /// A container the vendor names: the script exports `STADO_WORKER_NAME`
    /// from this environment variable, which the vendor sets to the
    /// machine's native id.
    ContainerEnv(&'static str),
    /// The vendor's API hands a new machine no startup script, so no agent
    /// can boot on it: Stado lists, reads and releases such machines and
    /// never dispatches to them.
    NoStartupScript,
}

impl VendorProfile {
    /// Whether Stado can boot an agent on a machine of this vendor.
    pub const fn boots_agent(&self) -> bool {
        !matches!(self.guest, GuestIdentity::NoStartupScript)
    }
}

/// Everything static Stado knows about one vendor.
#[derive(Clone, Copy, Debug)]
pub struct VendorProfile {
    pub vendor: GpuCloudVendor,
    /// One sentence for `stado capabilities`.
    pub summary: &'static str,
    /// The settings the adapter reads, under `<provider>.*`.
    pub config: &'static [ConfigField],
    /// The fields the credential item carries.
    pub credential_fields: &'static [&'static str],
    /// The vendor's instance types and the accelerator each carries, the
    /// preferred type for an accelerator first. The VRAM tier of each comes
    /// from the accelerator (`catalog::accel_vram_tier`).
    pub offers: &'static [(&'static str, &'static str)],
    pub guest: GuestIdentity,
}

/// The static profile of `vendor`.
pub const fn profile(vendor: GpuCloudVendor) -> &'static VendorProfile {
    match vendor {
        GpuCloudVendor::Arkane => &vendors::arkane::PROFILE,
        GpuCloudVendor::Crusoe => &vendors::crusoe::PROFILE,
        GpuCloudVendor::Cudo => &vendors::cudo::PROFILE,
        GpuCloudVendor::Hyperstack => &vendors::hyperstack::PROFILE,
        GpuCloudVendor::Lambda => &vendors::lambda::PROFILE,
        GpuCloudVendor::Latitude => &vendors::latitude::PROFILE,
        GpuCloudVendor::Nebius => &vendors::nebius::PROFILE,
        GpuCloudVendor::Oblivus => &vendors::oblivus::PROFILE,
        GpuCloudVendor::Oracle => &vendors::oracle::PROFILE,
        GpuCloudVendor::Runpod => &vendors::runpod::PROFILE,
        GpuCloudVendor::Salad => &vendors::salad::PROFILE,
        GpuCloudVendor::Scaleway => &vendors::scaleway::PROFILE,
        GpuCloudVendor::VastRental => &vendors::vast_rental::PROFILE,
        GpuCloudVendor::VoltagePark => &vendors::voltage_park::PROFILE,
        GpuCloudVendor::Vultr => &vendors::vultr::PROFILE,
    }
}

/// The API client of `vendor`. Construction reads nothing; credentials and
/// settings are read on the first call, so building the provider never fails
/// and a missing credential surfaces where it is used, by name.
pub fn api(vendor: GpuCloudVendor) -> Box<dyn GpuCloudApi> {
    match vendor {
        GpuCloudVendor::Arkane => Box::new(vendors::arkane::Api::new()),
        GpuCloudVendor::Crusoe => Box::new(vendors::crusoe::Api::new()),
        GpuCloudVendor::Cudo => Box::new(vendors::cudo::Api::new()),
        GpuCloudVendor::Hyperstack => Box::new(vendors::hyperstack::Api::new()),
        GpuCloudVendor::Lambda => Box::new(vendors::lambda::Api::new()),
        GpuCloudVendor::Latitude => Box::new(vendors::latitude::Api::new()),
        GpuCloudVendor::Nebius => Box::new(vendors::nebius::Api::new()),
        GpuCloudVendor::Oblivus => Box::new(vendors::oblivus::Api::new()),
        GpuCloudVendor::Oracle => Box::new(vendors::oracle::Api::new()),
        GpuCloudVendor::Runpod => Box::new(vendors::runpod::Api::new()),
        GpuCloudVendor::Salad => Box::new(vendors::salad::Api::new()),
        GpuCloudVendor::Scaleway => Box::new(vendors::scaleway::Api::new()),
        GpuCloudVendor::VastRental => Box::new(vendors::vast_rental::Api::new()),
        GpuCloudVendor::VoltagePark => Box::new(vendors::voltage_park::Api::new()),
        GpuCloudVendor::Vultr => Box::new(vendors::vultr::Api::new()),
    }
}
