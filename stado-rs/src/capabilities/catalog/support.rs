//! Support levels and the product capability rows that carry them.

use serde::Serialize;

use super::entries::CapabilityKind;
use super::providers::ProviderId;

/// User-facing, provider-independent feature support.
///
/// `Implemented` means Stado owns an operational adapter. `Partial` means the
/// user-facing contract is narrower than the capability description.
/// `External` records a dependency Stado can inspect or consume but does not
/// manage. `Planned` names a known provider equivalent without pretending that
/// an adapter exists. An omitted provider is `Unsupported`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySupport {
    Implemented,
    Partial,
    External,
    Planned,
    Unsupported,
}

impl CapabilitySupport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Partial => "partial",
            Self::External => "external",
            Self::Planned => "planned",
            Self::Unsupported => "unsupported",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderCapability {
    pub provider: ProviderId,
    pub support: CapabilitySupport,
    pub implementation: &'static str,
    pub note: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProductCapability {
    pub id: CapabilityKind,
    pub summary: &'static str,
    pub providers: &'static [ProviderCapability],
}

impl ProductCapability {
    pub fn support(self, provider: ProviderId) -> CapabilitySupport {
        self.providers
            .iter()
            .find(|entry| entry.provider == provider)
            .map(|entry| entry.support)
            .unwrap_or(CapabilitySupport::Unsupported)
    }
}
