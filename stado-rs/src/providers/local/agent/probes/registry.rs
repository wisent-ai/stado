//! Registry self-lookup (Python targets.load_targets(source="auto")).

use crate::targets::{self, ComputeTarget, Registry, RegistryError};

/// The registry document, GCS first with the bundled file as fallback
/// (Python `load_targets(source="auto")`). The fetch + 30 s TTL cache live
/// in [`targets`] — see `targets::fetch_registry_remote`.
pub async fn load_registry_auto() -> Result<Registry, RegistryError> {
    targets::load_registry_auto().await
}

/// Find the unique target declaring this host's identity.
/// Python `targets.lookup_self(hostname, source="auto")`.
pub async fn lookup_self_auto(hostname: &str) -> Result<Option<ComputeTarget>, RegistryError> {
    let registry = load_registry_auto().await?;
    Ok(registry.lookup_self(hostname)?.cloned())
}

/// Return the named target (Python `targets.lookup(name, source="auto")`).
pub async fn lookup_auto(name: &str) -> Result<Option<ComputeTarget>, RegistryError> {
    let registry = load_registry_auto().await?;
    Ok(registry.lookup(name).cloned())
}
