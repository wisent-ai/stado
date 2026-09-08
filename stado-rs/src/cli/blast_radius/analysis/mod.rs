//! Dependency and impact analysis: which declared capability failed, whether
//! it owns the configured backends, and what that costs downstream.

mod coverage;
mod domains;
mod downstream;

use crate::cli::CmdError;

pub(super) use coverage::compare_coverage;
pub(super) use domains::data_domains;
pub(super) use downstream::downstream_impacts;

pub(in crate::cli::blast_radius) fn validate_dependency(
    dependency: &str,
) -> Result<&'static crate::capabilities::CapabilityVariant, CmdError> {
    if let Some(variant) = crate::capabilities::configurable_variant(
        crate::capabilities::RuntimeFacet::Dependency,
        dependency,
    ) {
        return Ok(variant);
    }
    let choices =
        crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Dependency)
            .collect::<Vec<_>>()
            .join(", ");
    Err(CmdError::click(format!(
        "unknown dependency {dependency:?}; use one of: {choices}"
    )))
}

pub(in crate::cli::blast_radius) fn dependency_owns_backend(
    dependency: &crate::capabilities::CapabilityVariant,
    backend: &str,
) -> bool {
    let backend_owner =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Storage, backend)
            .and_then(|variant| variant.provider);
    dependency.provider.is_some() && dependency.provider == backend_owner
}

fn dependency_owns_release(dependency: &crate::capabilities::CapabilityVariant, url: &str) -> bool {
    dependency
        .provider
        .is_some_and(|provider| provider.owns_release_url(url))
}
