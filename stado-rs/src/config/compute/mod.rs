//! Compute provider selection and per-provider settings.

use std::sync::LazyLock;

use crate::config::{canonicalize_capability_names, DEFAULT_PROVIDERS};
use crate::config_file::resolve_list as cfg_list;

mod aws;
mod azure;
mod gcp;
mod zones;

pub use aws::*;
pub use azure::*;
pub use gcp::*;
pub use zones::*;

static WC_DISABLED_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Compute,
        cfg_list(
            crate::capabilities::DISABLED_PROVIDERS_CONFIG.env,
            crate::capabilities::DISABLED_PROVIDERS_CONFIG.path,
            &[],
        ),
    )
});
static WC_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut providers = canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Compute,
        cfg_list(
            crate::capabilities::PROVIDERS_CONFIG.env,
            crate::capabilities::PROVIDERS_CONFIG.path,
            DEFAULT_PROVIDERS,
        ),
    );
    providers.retain(|provider| !WC_DISABLED_PROVIDERS.contains(provider));
    providers
});

/// Multi-provider dispatch (env `WC_PROVIDERS`, comma-separated).
/// Coordinator and Cloud Function ticks iterate this list, calling
/// check_running_jobs / reap_dead_agents / schedule_queued_jobs per
/// provider. A provider whose constructor throws (creds missing) is logged
/// and skipped. An unconfigured deployment has no provider rather than a
/// hidden GCP dependency.
pub fn wc_providers() -> &'static [String] {
    &WC_PROVIDERS
}

/// Explicitly disabled entries from the configured provider preference order.
/// Keeping this separate from [`wc_providers`] lets deployment config explain
/// why a provisioned provider is fenced without letting the scheduler call it.
pub fn wc_disabled_providers() -> &'static [String] {
    &WC_DISABLED_PROVIDERS
}

/// One scalar setting a GPU cloud vendor adapter declares in its catalog
/// configuration (`<provider>.<key>`, environment `<PROVIDER>_<KEY>`). Empty
/// when neither the profile nor the environment carries it: the adapter
/// decides whether that is a refusal, and says which path to set.
pub fn gpu_cloud_setting(provider: crate::capabilities::ProviderId, key: &str) -> String {
    crate::config::resolve_compute_binding(provider, key, "")
}
