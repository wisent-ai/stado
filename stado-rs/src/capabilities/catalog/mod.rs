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

pub fn capability_support(capability: CapabilityKind, provider: ProviderId) -> CapabilitySupport {
    CAPABILITIES
        .iter()
        .find(|entry| entry.id == capability)
        .map(|entry| entry.support(provider))
        .unwrap_or(CapabilitySupport::Unsupported)
}

pub fn capabilities_for_provider(
    provider: ProviderId,
) -> impl Iterator<Item = &'static ProductCapability> {
    CAPABILITIES
        .iter()
        .filter(move |capability| capability.support(provider) != CapabilitySupport::Unsupported)
}
