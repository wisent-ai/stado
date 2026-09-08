//! Validate, verify and atomically publish a manifest.

use crate::artifacts_models::{
    ArtifactError, ArtifactManifest, ArtifactVerification, VerificationReport,
};

use super::super::adapters::get_adapter;
use super::super::validation::validate_manifest;
use super::{actor, alias_path, manifest_path, now, ArtifactRegistry, RegistryError};

impl ArtifactRegistry {
    /// Validate, verify and atomically publish a manifest. Python
    /// `ArtifactRegistry.publish`.
    pub async fn publish(
        &self,
        manifest: &ArtifactManifest,
        verify: bool,
        full: bool,
    ) -> Result<ArtifactManifest, RegistryError> {
        if self
            .store
            .download_text(&alias_path(&manifest.ref_))
            .await?
            .is_some()
        {
            return Err(ArtifactError::new(
                "ARTIFACT_VERSION_CONFLICT",
                format!(
                    "artifact version collides with an existing alias: {}",
                    manifest.ref_
                ),
            )
            .into());
        }
        let mut issues = validate_manifest(manifest);
        let adapter = get_adapter(&manifest.ref_.r#type);
        let mut report = VerificationReport {
            adapter: "generic-v1".to_string(),
            passed: issues.is_empty(),
            issues: issues.clone(),
            summary: Default::default(),
        };
        if let Some(adapter) = &adapter {
            if issues.is_empty() && verify {
                report = adapter.verify(manifest, full).await;
                issues.extend(report.issues.iter().cloned());
            }
        }
        if !report.passed && issues.is_empty() {
            issues.push(format!("{} verification failed", report.adapter));
        }
        if !issues.is_empty() {
            return Err(
                ArtifactError::new("ARTIFACT_VERIFICATION_FAILED", issues.join("; ")).into(),
            );
        }

        let mut prepared = manifest.clone();
        if prepared.created_at.is_empty() {
            prepared.created_at = now();
        }
        if prepared.created_by.is_empty() {
            prepared.created_by = actor();
        }
        prepared.verification = ArtifactVerification {
            adapter: report.adapter.clone(),
            verified_at: if verify { now() } else { String::new() },
            result: if verify { "passed" } else { "skipped" }.to_string(),
            manifest_sha256: String::new(),
            issues: report.issues.clone(),
        };
        if !report.summary.is_empty() {
            for (key, value) in &report.summary {
                prepared.summary.insert(key.clone(), value.clone());
            }
        }
        // Python hashes the canonical document with manifest_sha256
        // blanked, then stores the document with the digest filled in.
        let digest = prepared.manifest_sha256();
        prepared.verification.manifest_sha256 = digest;

        let content = prepared.to_json();
        let path = manifest_path(&prepared.ref_);
        if self.store.create_text_if_absent(&path, &content).await? {
            return Ok(prepared);
        }
        let existing = self.store.download_text(&path).await?;
        if existing.as_deref() == Some(content.as_str()) {
            return Ok(prepared);
        }
        Err(ArtifactError::new(
            "ARTIFACT_VERSION_CONFLICT",
            format!(
                "immutable artifact version already exists with different content: {}",
                prepared.ref_
            ),
        )
        .into())
    }
}
