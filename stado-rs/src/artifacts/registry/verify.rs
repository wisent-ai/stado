//! Re-run generic and type-specific verification for a stored artifact.

use crate::artifacts_models::{ArtifactRef, VerificationReport};

use super::super::adapters::get_adapter;
use super::super::validation::validate_manifest;
use super::{ArtifactRegistry, RegistryError};

impl ArtifactRegistry {
    /// Re-run generic + type-specific verification. Python
    /// `ArtifactRegistry.verify`.
    pub async fn verify(
        &self,
        reference: &ArtifactRef,
        full: bool,
    ) -> Result<VerificationReport, RegistryError> {
        let manifest = self.resolve_manifest(reference).await?;
        let issues = validate_manifest(&manifest);
        if !issues.is_empty() {
            return Ok(VerificationReport {
                adapter: "generic-v1".to_string(),
                passed: false,
                issues,
                summary: Default::default(),
            });
        }
        let Some(adapter) = get_adapter(&manifest.ref_.r#type) else {
            return Ok(VerificationReport {
                adapter: "generic-v1".to_string(),
                passed: true,
                issues: Vec::new(),
                summary: Default::default(),
            });
        };
        Ok(adapter.verify(&manifest, full).await)
    }
}
