//! Fetch, resolve and list manifests.

use serde_json::Value;

use crate::artifacts_models::{ArtifactError, ArtifactManifest, ArtifactRef};

use super::{
    alias_path, manifest_path, stringify_json_scalar, ArtifactRegistry, RegistryError,
    MANIFEST_PREFIX,
};

impl ArtifactRegistry {
    /// Fetch a manifest by exact version ref. Python `ArtifactRegistry.get`.
    pub async fn get(&self, reference: &ArtifactRef) -> Result<ArtifactManifest, RegistryError> {
        let raw = self.store.download_text(&manifest_path(reference)).await?;
        let Some(raw) = raw else {
            return Err(ArtifactError::new(
                "ARTIFACT_NOT_FOUND",
                format!("artifact not found: {reference}"),
            )
            .into());
        };
        let manifest = ArtifactManifest::from_json(&raw)?;
        if &manifest.ref_ != reference {
            return Err(ArtifactError::new(
                "ARTIFACT_CORRUPT_MANIFEST",
                format!("manifest identity does not match its storage path: {reference}"),
            )
            .into());
        }
        Ok(manifest)
    }

    /// Resolve a version-or-alias ref to the immutable version ref.
    /// Python `ArtifactRegistry.resolve`.
    pub async fn resolve(&self, reference: &ArtifactRef) -> Result<ArtifactRef, RegistryError> {
        if self
            .store
            .download_text(&manifest_path(reference))
            .await?
            .is_some()
        {
            self.get(reference).await?;
            return Ok(reference.clone());
        }
        let raw = self.store.download_text(&alias_path(reference)).await?;
        let Some(raw) = raw else {
            return Err(ArtifactError::new(
                "ARTIFACT_NOT_FOUND",
                format!("artifact or alias not found: {reference}"),
            )
            .into());
        };
        let corrupt = |exc: String| {
            ArtifactError::new(
                "ARTIFACT_CORRUPT_ALIAS",
                format!("invalid alias record for {reference}: {exc}"),
            )
        };
        let alias: Value = serde_json::from_str(&raw).map_err(|exc| corrupt(exc.to_string()))?;
        let target_version = alias
            .get("target_version")
            .map(stringify_json_scalar)
            .ok_or_else(|| corrupt("missing 'target_version'".to_string()))?;
        let target = reference.with_version(&target_version)?;
        self.get(&target).await?;
        Ok(target)
    }

    /// `get(resolve(ref))` — Python `resolve_manifest`.
    pub async fn resolve_manifest(
        &self,
        reference: &ArtifactRef,
    ) -> Result<ArtifactManifest, RegistryError> {
        self.get(&self.resolve(reference).await?).await
    }

    /// List manifests, newest first (Python sorts by
    /// `(created_at, str(ref))` descending). Filters match Python
    /// `ArtifactRegistry.list`: empty `type_name`/`namespace`/`name` widen
    /// the scanned prefix progressively; `labels` must match exactly.
    pub async fn list(
        &self,
        type_name: &str,
        namespace: &str,
        name: &str,
        labels: &[(String, String)],
    ) -> Result<Vec<ArtifactManifest>, RegistryError> {
        let mut parts = vec![MANIFEST_PREFIX.to_string()];
        for value in [type_name, namespace, name] {
            if value.is_empty() {
                break;
            }
            parts.push(value.to_string());
        }
        let prefix = format!("{}/", parts.join("/"));
        let mut manifests: Vec<ArtifactManifest> = Vec::new();
        for path in self.store.list_paths(&prefix, 0).await? {
            if !path.ends_with(".json") {
                continue;
            }
            let Some(raw) = self.store.download_text(&path).await? else {
                continue;
            };
            let manifest = ArtifactManifest::from_json(&raw)?;
            if !type_name.is_empty() && manifest.ref_.r#type != type_name {
                continue;
            }
            if !namespace.is_empty() && manifest.ref_.namespace != namespace {
                continue;
            }
            if !name.is_empty() && manifest.ref_.name != name {
                continue;
            }
            if labels
                .iter()
                .any(|(key, value)| manifest.labels.get(key) != Some(value))
            {
                continue;
            }
            manifests.push(manifest);
        }
        manifests.sort_by(|a, b| {
            let key_a = (&a.created_at, a.ref_.to_string());
            let key_b = (&b.created_at, b.ref_.to_string());
            key_b.cmp(&key_a)
        });
        Ok(manifests)
    }
}
