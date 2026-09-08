//! Provider resolution pass — turn `WC_PROVIDERS` into the tick's arms.

use std::sync::Arc;

use crate::config;
use crate::coordinator::log;
use crate::providers::{get_provider, BoxProvider, Provider};

/// One resolved provider arm of the tick. Box providers need their
/// concrete type for [`run_box_tick`](crate::scheduler::dispatch::r#box::run_box_tick), so they cannot hide behind
/// `Arc<dyn Provider>` here.
pub enum ResolvedProvider {
    /// A cloud VM provider (gcp/aws/azure): check + reap + schedule.
    Cloud {
        /// Provider name from `WC_PROVIDERS` (also the reaper `kind`).
        name: String,
        /// The provider client.
        provider: Arc<dyn Provider>,
    },
    /// A Box provider (box/box-ascii): the lease state machine tick.
    Box {
        /// Provider name from `WC_PROVIDERS`.
        name: String,
        /// The concrete box provider.
        provider: Arc<BoxProvider>,
    },
}

/// Resolve `WC_PROVIDERS` into tick arms. "local" is skipped (device-local
/// agents claim assigned jobs directly; there is no cloud VM lifecycle to
/// schedule or reap for that provider). A constructor failure is logged
/// and skipped so a misconfigured provider never blocks the primary one
/// (Python wraps the box arm in try/except; the cloud arms construct
/// lazily and cannot fail here).
pub fn resolve_providers() -> Vec<ResolvedProvider> {
    let mut out = Vec::new();
    for name in config::wc_providers() {
        let Some(variant) =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Compute, name)
        else {
            log(&format!(
                "provider {name} tick failed: capability is not registered"
            ));
            continue;
        };
        match variant.adapter {
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::ExistingHost,
            ) => continue,
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::Box,
            ) => match BoxProvider::from_env() {
                Ok(provider) => out.push(ResolvedProvider::Box {
                    name: variant.id.to_string(),
                    provider: Arc::new(provider),
                }),
                Err(exc) => log(&format!("provider {} tick failed: {exc}", variant.id)),
            },
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::Gcp
                | crate::capabilities::ComputeAdapter::Aws
                | crate::capabilities::ComputeAdapter::Azure,
            ) => match get_provider(variant.id) {
                Ok(provider) => out.push(ResolvedProvider::Cloud {
                    name: variant.id.to_string(),
                    provider,
                }),
                Err(exc) => log(&format!("provider {} tick failed: {exc}", variant.id)),
            },
            _ => log(&format!(
                "provider {} tick failed: no coordinator adapter",
                variant.id
            )),
        }
    }
    out
}
