//! The adapter contract and the static registry that resolves an artifact
//! type to the adapter that verifies it.

use async_trait::async_trait;

use super::ActivationDatasetAdapter;
use crate::artifacts_models::{ArtifactManifest, VerificationReport};

/// Python `ArtifactAdapter` (Protocol).
#[async_trait]
pub trait ArtifactAdapter: Send + Sync {
    fn type_name(&self) -> &'static str;
    fn adapter_name(&self) -> &'static str;
    /// `full` requests the adapter's exhaustive verification; the
    /// activation adapter currently runs its inventory check either way
    /// (Python parity — `full` is accepted but unused there).
    async fn verify(&self, manifest: &ArtifactManifest, full: bool) -> VerificationReport;
}

/// The static adapter registry (Python `get_adapter` over `_ADAPTERS`).
/// `None` for artifact types with no type-specific verification — the
/// registry then falls back to the `generic-v1` report.
pub fn get_adapter(type_name: &str) -> Option<Box<dyn ArtifactAdapter>> {
    match type_name {
        "activation-dataset" => Some(Box::new(ActivationDatasetAdapter::new())),
        _ => None,
    }
}
