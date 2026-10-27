//! The product capability catalog: provider names, support levels, and the
//! single declaration of every user-facing capability.

mod entries;
mod macros;
mod providers;
mod support;

pub use entries::{CapabilityKind, CAPABILITIES};
pub use providers::{provider, ProviderId, PROVIDERS};
pub use support::{CapabilitySupport, ProductCapability, ProviderCapability};

pub fn product_capabilities() -> &'static [ProductCapability] {
    CAPABILITIES
}

pub fn product_capability(id: &str) -> Option<&'static ProductCapability> {
    CAPABILITIES
        .iter()
        .find(|capability| capability.id.as_str() == id)
}

/// What the catalog declares for `capability` on `provider`; a capability
/// the catalog does not declare is supported by no provider.
pub fn capability_support(capability: CapabilityKind, provider: ProviderId) -> CapabilitySupport {
    match CAPABILITIES.iter().find(|entry| entry.id == capability) {
        Some(entry) => entry.support(provider),
        None => CapabilitySupport::Unsupported,
    }
}

pub fn capabilities_for_provider(
    provider: ProviderId,
) -> impl Iterator<Item = &'static ProductCapability> {
    CAPABILITIES
        .iter()
        .filter(move |capability| capability.support(provider) != CapabilitySupport::Unsupported)
}
