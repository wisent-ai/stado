//! Mutable aliases: compare-and-swap retargeting and reverse lookup.

use serde_json::Value;

use crate::artifacts_models::{ArtifactError, ArtifactRef};
use crate::queue::StorageError;

use super::{
    actor, alias_path, manifest_path, now, stringify_json_scalar, ArtifactRegistry, RegistryError,
    ALIAS_PREFIX,
};

impl ArtifactRegistry {
    /// Create or update a mutable alias pointing at an immutable version.
    /// Python `ArtifactRegistry.set_alias`: same-target updates are
    /// idempotent; retargeting requires `expected_previous` (optimistic
    /// precondition) and commits through CAS.
    pub async fn set_alias(
        &self,
        target: &ArtifactRef,
        alias: &str,
        expected_previous: Option<&str>,
        updated_by: &str,
    ) -> Result<ArtifactRef, RegistryError> {
        self.get(target).await?;
        let alias_ref = target.with_version(alias)?;
        if self
            .store
            .download_text(&manifest_path(&alias_ref))
            .await?
            .is_some()
        {
            return Err(ArtifactError::new(
                "ARTIFACT_ALIAS_CONFLICT",
                format!("alias name collides with immutable artifact version: {alias_ref}"),
            )
            .into());
        }
        let path = alias_path(&alias_ref);
        let record = serde_json::json!({
            "schema_version":
                1,
            "ref": alias_ref.coordinate(),
            "alias": alias,
            "target_version": target.version,
            "updated_at": now(),
            "updated_by": if updated_by.is_empty() { actor() } else { updated_by.to_string() },
            "previous_version": expected_previous.unwrap_or(""),
        });
        let content = crate::queue::submit::json_dumps_sorted_compact(&record);
        let current = self.store.read_text_versioned(&path).await?;
        let Some(current) = current else {
            if expected_previous.is_some_and(|previous| !previous.is_empty()) {
                return Err(ArtifactError::new(
                    "ARTIFACT_ALIAS_CONFLICT",
                    format!(
                        "alias {alias_ref} does not exist; expected {}",
                        expected_previous.unwrap_or_default()
                    ),
                )
                .into());
            }
            if !self.store.create_text_if_absent(&path, &content).await? {
                return Err(ArtifactError::new(
                    "ARTIFACT_ALIAS_CONFLICT",
                    format!("alias was created concurrently: {alias_ref}"),
                )
                .into());
            }
            return Ok(alias_ref);
        };

        let corrupt = |exc: String| {
            ArtifactError::new(
                "ARTIFACT_CORRUPT_ALIAS",
                format!("invalid alias record for {alias_ref}: {exc}"),
            )
        };
        let current_record: Value =
            serde_json::from_str(&current.content).map_err(|exc| corrupt(exc.to_string()))?;
        let current_target = current_record
            .get("target_version")
            .map(stringify_json_scalar)
            .ok_or_else(|| corrupt("missing 'target_version'".to_string()))?;
        if current_target == target.version {
            return Ok(alias_ref);
        }
        let Some(expected_previous) = expected_previous else {
            return Err(ArtifactError::new(
                "ARTIFACT_ALIAS_CONFLICT",
                format!(
                    "alias {alias_ref} currently targets {current_target}; pass expected_previous"
                ),
            )
            .into());
        };
        if current_target != expected_previous {
            return Err(ArtifactError::new(
                "ARTIFACT_ALIAS_CONFLICT",
                format!(
                    "alias {alias_ref} targets {current_target}, not expected {expected_previous}"
                ),
            )
            .into());
        }
        self.store
            .compare_and_swap_text(&path, &current.version, &content)
            .await
            .map_err(|exc| match exc {
                StorageError::StorageConflict(_) => RegistryError::Artifact(ArtifactError::new(
                    "ARTIFACT_ALIAS_CONFLICT",
                    format!("alias changed concurrently: {alias_ref}"),
                )),
                other => RegistryError::Storage(other),
            })?;
        Ok(alias_ref)
    }

    /// Aliases (sorted) currently pointing at this exact version. Python
    /// `ArtifactRegistry.aliases_for`.
    pub async fn aliases_for(&self, reference: &ArtifactRef) -> Result<Vec<String>, RegistryError> {
        let prefix = format!(
            "{ALIAS_PREFIX}/{}/{}/{}/",
            reference.r#type, reference.namespace, reference.name
        );
        let mut aliases: Vec<String> = Vec::new();
        for path in self.store.list_paths(&prefix, 0).await? {
            let Some(raw) = self.store.download_text(&path).await? else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if value.get("target_version").and_then(Value::as_str)
                == Some(reference.version.as_str())
            {
                let alias = value
                    .get("alias")
                    .map(stringify_json_scalar)
                    .filter(|alias| !alias.is_empty())
                    .unwrap_or_else(|| {
                        path.rsplit('/')
                            .next()
                            .unwrap_or("")
                            .trim_end_matches(".json")
                            .to_string()
                    });
                aliases.push(alias);
            }
        }
        aliases.sort();
        Ok(aliases)
    }
}
