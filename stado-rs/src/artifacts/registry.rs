//! Immutable manifests and atomic mutable aliases over [`JobStorage`].
//!
//! Port of `stado/artifacts/registry.py`. Layout on the blob backend:
//! manifests are immutable documents at
//! `artifacts/manifests/<type>/<namespace>/<name>/<version>.json` (create-
//! if-absent; re-publish of identical content is idempotent, changed
//! content is `ARTIFACT_VERSION_CONFLICT`), aliases are mutable one-file
//! records at `artifacts/aliases/...` updated through compare-and-swap.

use serde_json::Value;

use crate::artifacts_models::{
    ArtifactError, ArtifactManifest, ArtifactRef, ArtifactVerification, VerificationReport,
};
use crate::queue::{JobStorage, StorageError};

use super::adapters::get_adapter;
use super::validation::validate_manifest;

const MANIFEST_PREFIX: &str = "artifacts/manifests";
const ALIAS_PREFIX: &str = "artifacts/aliases";

fn manifest_path(reference: &ArtifactRef) -> String {
    format!(
        "{MANIFEST_PREFIX}/{}/{}/{}/{}.json",
        reference.r#type, reference.namespace, reference.name, reference.version
    )
}

fn alias_path(reference: &ArtifactRef) -> String {
    format!(
        "{ALIAS_PREFIX}/{}/{}/{}/{}.json",
        reference.r#type, reference.namespace, reference.name, reference.version
    )
}

/// Python `_now()`: `datetime.now(timezone.utc).isoformat()`.
fn now() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Python `_actor()`: `f"{getpass.getuser()}@{socket.gethostname()}"`.
fn actor() -> String {
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("LOGNAME").ok())
        .unwrap_or_default();
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_default();
    format!("{user}@{host}")
}

/// Registry operation failure. [`RegistryError::Artifact`] is the
/// machine-readable Python `ArtifactError`; [`RegistryError::Storage`] is
/// an underlying blob-backend failure (Python lets those propagate).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Artifact(#[from] ArtifactError),
    #[error("{0}")]
    Storage(#[from] StorageError),
}

/// The artifact registry facade (Python `ArtifactRegistry`). Cheap to
/// clone — it only holds the [`JobStorage`] handle.
#[derive(Clone)]
pub struct ArtifactRegistry {
    store: JobStorage,
}

impl ArtifactRegistry {
    /// Registry over the configured storage (Python
    /// `ArtifactRegistry()` → `JobStorage(BUCKET)`).
    pub async fn new() -> Result<Self, StorageError> {
        Ok(Self {
            store: JobStorage::new().await?,
        })
    }

    /// Registry over an explicit store (tests, custom deployments).
    pub fn with_store(store: JobStorage) -> Self {
        Self { store }
    }

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
            "schema_version": 1,
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

/// Python `str(value)` for JSON scalars (used where Python stringifies
/// `alias["target_version"]` etc.). Numbers render without a trailing
/// `.0`, matching Python's `str(int)` for integral values.
fn stringify_json_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

