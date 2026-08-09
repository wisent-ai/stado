//! Checked-in product release contract and durable pipeline records.
//!
//! `.wisent-release.json` is the host-independent boundary between a product
//! and Stado.  Repository locations, fleet hosts, storage providers and secret
//! material deliberately do not appear in this schema.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PRODUCT_MANIFEST: &str = ".wisent-release.json";
pub const SCHEMA_VERSION: u32 = 1;
pub const RUNNER_PLATFORMS: [&str; 2] = ["darwin-arm64", "linux-amd64"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProductManifest {
    Release(ReleasePipelineManifest),
    NonRelease(NonReleaseManifest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonReleaseManifest {
    pub schema_version: u32,
    pub product: String,
    pub releases: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePipelineManifest {
    pub schema_version: u32,
    pub product: String,
    pub releases: bool,
    pub version_source: VersionSource,
    pub platforms: BTreeMap<String, PlatformRecipe>,
    #[serde(default)]
    pub runtime: Option<RuntimeContract>,
    pub promotion: PromotionPolicy,
    #[serde(default)]
    pub mirrors: Vec<Mirror>,
    #[serde(default)]
    pub inputs: BTreeMap<String, ReleaseInput>,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VersionSource {
    Json { path: String, pointer: String },
    Regex { path: String, pattern: String },
    Text { path: String },
}

impl VersionSource {
    fn path(&self) -> &str {
        match self {
            Self::Json { path, .. } | Self::Regex { path, .. } | Self::Text { path } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRecipe {
    pub runner_platform: String,
    pub quality: Vec<QualityGate>,
    pub build: BuildCommand,
    pub stage: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityGate {
    pub name: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildCommand {
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInput {
    pub uri: String,
    pub sha256: String,
    pub mount: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeContract {
    pub binary: String,
    pub launcher: String,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: String,
    #[serde(default)]
    pub rollback_compatible_with: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionPolicy {
    pub channels: Vec<PipelineChannel>,
    pub reconcile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineChannel {
    Candidate,
    Stable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mirror {
    pub name: String,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub name: String,
    pub platform: String,
    pub argv: Vec<String>,
    pub required: bool,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSourceIdentity {
    pub commit: String,
    pub source_sha256: String,
    pub source_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCatalogEntry {
    pub schema_version: u32,
    pub product: String,
    pub manifest_sha256: String,
    pub manifest: ProductManifest,
    #[serde(default)]
    pub source: Option<CatalogSourceIdentity>,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRunState {
    Submitting,
    Waiting,
    Publishing,
    Delivering,
    Promoted,
    Reconciled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformRunState {
    Submitted,
    Qualified,
    Published,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRun {
    pub platform: String,
    pub builder: String,
    pub job_id: String,
    pub output_prefix: String,
    pub state: PlatformRunState,
    #[serde(default)]
    pub artifact_sha256: Option<String>,
    #[serde(default)]
    pub release_manifest_sha256: Option<String>,
    #[serde(default)]
    pub qualification_uri: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRunState {
    Submitted,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRun {
    pub name: String,
    pub platform: String,
    pub job_id: String,
    pub output_prefix: String,
    pub required: bool,
    pub state: DeliveryRunState,
    #[serde(default)]
    pub receipt_sha256: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRun {
    pub schema_version: u32,
    pub run_id: String,
    pub product: String,
    pub version: String,
    pub channel: PipelineChannel,
    pub source_commit: String,
    pub source_sha256: String,
    pub source_uri: String,
    pub manifest_sha256: String,
    pub manifest_uri: String,
    pub state: ReleaseRunState,
    pub platforms: BTreeMap<String, PlatformRun>,
    #[serde(default)]
    pub deliveries: BTreeMap<String, DeliveryRun>,
    #[serde(default)]
    pub failure: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub schema_version: u32,
    pub run_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub builder: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub manifest_sha256: String,
    pub source_archive: String,
    pub manifest_path: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, WorkerInput>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerInput {
    pub uri: String,
    pub sha256: String,
    pub archive_path: String,
    pub mount: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReceipt {
    pub name: String,
    pub argv: Vec<String>,
    pub status: StepStatus,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub sha256: String,
    pub bytes: u64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReceipt {
    pub schema_version: u32,
    pub run_id: String,
    pub job_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub builder: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub manifest_sha256: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, ReceiptInput>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
    pub quality: Vec<StepReceipt>,
    pub build: StepReceipt,
    pub status: StepStatus,
    #[serde(default)]
    pub artifact: Option<ArtifactReceipt>,
    pub completed_at: String,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptInput {
    pub uri: String,
    pub sha256: String,
    pub mount: String,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn platform_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
}

fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

pub fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.chars().any(char::is_control)
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn argv(value: &[String]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|part| !part.is_empty() && !part.as_bytes().contains(&0))
}

pub fn parse_product_manifest(bytes: &[u8]) -> Result<ProductManifest, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{PRODUCT_MANIFEST}: invalid JSON: {error}"))?;
    let releases = value
        .get("releases")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{PRODUCT_MANIFEST}: releases must be true or false"))?;
    let manifest = if releases {
        ProductManifest::Release(
            serde_json::from_value(value)
                .map_err(|error| format!("{PRODUCT_MANIFEST}: {error}"))?,
        )
    } else {
        ProductManifest::NonRelease(
            serde_json::from_value(value)
                .map_err(|error| format!("{PRODUCT_MANIFEST}: {error}"))?,
        )
    };
    validate_product_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_product_manifest(manifest: &ProductManifest) -> Result<(), String> {
    match manifest {
        ProductManifest::NonRelease(value) => {
            if value.schema_version != SCHEMA_VERSION || value.releases {
                return Err(
                    "non-release manifest must declare schema_version 1 and releases false".into(),
                );
            }
            if !identifier(&value.product)
                || value.reason.trim().is_empty()
                || value.reason.chars().any(char::is_control)
            {
                return Err("non-release manifest product or reason is invalid".into());
            }
            Ok(())
        }
        ProductManifest::Release(value) => validate_release_manifest(value),
    }
}

pub fn validate_release_manifest(manifest: &ReleasePipelineManifest) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION || !manifest.releases {
        return Err("release manifest must declare schema_version 1 and releases true".into());
    }
    if !identifier(&manifest.product) {
        return Err("release manifest product is not a canonical identifier".into());
    }
    if !safe_relative(manifest.version_source.path()) {
        return Err("release manifest version_source path must be repository-relative".into());
    }
    match &manifest.version_source {
        VersionSource::Json { pointer, .. } if !pointer.starts_with('/') => {
            return Err("JSON version_source pointer must be an absolute JSON pointer".into())
        }
        VersionSource::Regex { pattern, .. } => {
            let expression = Regex::new(pattern)
                .map_err(|error| format!("version_source regex is invalid: {error}"))?;
            if expression
                .capture_names()
                .filter(|name| *name == Some("version"))
                .count()
                != 1
            {
                return Err(
                    "regex version_source must contain exactly one capture named version".into(),
                );
            }
        }
        _ => {}
    }
    if manifest.platforms.is_empty() {
        return Err("release manifest platforms must not be empty".into());
    }
    for (platform, recipe) in &manifest.platforms {
        if !platform_identifier(platform)
            || !RUNNER_PLATFORMS.contains(&recipe.runner_platform.as_str())
        {
            return Err(format!(
                "{platform:?}: invalid output platform or runner_platform"
            ));
        }
        let mut gates = BTreeSet::new();
        for gate in &recipe.quality {
            if !identifier(&gate.name) || !gates.insert(gate.name.as_str()) || !argv(&gate.argv) {
                return Err(format!(
                    "{platform}: quality gates require unique names and non-empty argv"
                ));
            }
        }
        for (name, reference) in &recipe.secret_env {
            let Some((item, field)) = reference.split_once('#') else {
                return Err(format!(
                    "{platform}: secret_env must use item#field references"
                ));
            };
            if !env_name(name) || !identifier(item) || !identifier(field) {
                return Err(format!("{platform}: secret_env is invalid"));
            }
        }
        if !argv(&recipe.build.argv) || recipe.stage.is_empty() {
            return Err(format!(
                "{platform}: build argv and stage mapping must not be empty"
            ));
        }
        let mut destinations = BTreeSet::new();
        for (source, destination) in &recipe.stage {
            if !safe_relative(source)
                || !safe_relative(destination)
                || !destinations.insert(destination.as_str())
            {
                return Err(format!("{platform}: stage paths are unsafe or duplicate"));
            }
        }
        if let Some(runtime) = &manifest.runtime {
            if !destinations.contains(runtime.binary.as_str())
                || !destinations.contains(runtime.launcher.as_str())
            {
                return Err(format!(
                    "{platform}: runtime binary and launcher must be staged destinations"
                ));
            }
        }
    }
    if manifest.promotion.reconcile && manifest.runtime.is_none() {
        return Err("promotion.reconcile=true requires a runtime contract".into());
    }
    if let Some(runtime) = &manifest.runtime {
        if !safe_relative(&runtime.binary)
            || !safe_relative(&runtime.launcher)
            || runtime.config_schema == 0
            || runtime.state_schema == 0
            || !identifier(&runtime.minimum_stado_version)
        {
            return Err("release runtime contract is invalid".into());
        }
        let mut rollback = BTreeSet::new();
        if runtime
            .rollback_compatible_with
            .iter()
            .any(|version| !identifier(version) || !rollback.insert(version))
        {
            return Err("rollback_compatible_with contains an invalid or duplicate version".into());
        }
    }
    let channels: BTreeSet<_> = manifest.promotion.channels.iter().copied().collect();
    let mut mounts: Vec<&str> = Vec::new();
    let mut environment_names = BTreeSet::new();
    for (name, input) in &manifest.inputs {
        let env_name = name
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    byte.to_ascii_uppercase() as char
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let object = crate::object_store::ObjectRef::parse(&input.uri)
            .map_err(|error| format!("release input {name:?} URI is invalid: {error}"))?;
        let immutable_source = object.namespace() == "sources"
            && object.key().split('/').any(|part| part == input.sha256);
        let immutable_release =
            object.namespace() == "releases" && object.key().split('/').count() >= 4;
        if !identifier(name)
            || !environment_names.insert(env_name)
            || !sha256(&input.sha256)
            || !safe_relative(&input.mount)
            || (!immutable_source && !immutable_release)
            || mounts.iter().any(|existing| {
                input.mount == *existing
                    || input.mount.starts_with(&format!("{existing}/"))
                    || existing.starts_with(&format!("{}/", input.mount))
            })
        {
            return Err(format!(
                "release input {name:?} must use an immutable Stado URI, exact digest, and unique non-overlapping mount"
            ));
        }
        mounts.push(&input.mount);
    }
    if channels.len() != manifest.promotion.channels.len() || channels.is_empty() {
        return Err("promotion must contain unique channels".into());
    }
    let mut deliveries = BTreeSet::new();
    for delivery in &manifest.deliveries {
        if !identifier(&delivery.name)
            || !deliveries.insert(delivery.name.as_str())
            || !manifest.platforms.contains_key(&delivery.platform)
            || !argv(&delivery.argv)
        {
            return Err(
                "deliveries require unique names, declared platforms, and non-empty argv".into(),
            );
        }
        let mut secret_names = BTreeSet::new();
        for (name, reference) in &delivery.secret_env {
            let Some((item, field)) = reference.split_once('#') else {
                return Err(format!(
                    "delivery {} secret_env must use item#field references",
                    delivery.name
                ));
            };
            if !env_name(name)
                || !secret_names.insert(name)
                || !identifier(item)
                || !identifier(field)
            {
                return Err(format!("delivery {} secret_env is invalid", delivery.name));
            }
        }
    }
    let mut mirrors = BTreeSet::new();
    for mirror in &manifest.mirrors {
        if !identifier(&mirror.name)
            || !mirrors.insert(mirror.name.as_str())
            || crate::object_store::ObjectRef::parse(&mirror.uri).is_err()
        {
            return Err("mirrors require unique names and provider-neutral stado:// URIs".into());
        }
    }
    Ok(())
}

pub fn declared_version(root: &Path, source: &VersionSource) -> Result<String, String> {
    let path = root.join(source.path());
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("cannot read version source {}: {error}", path.display()))?;
    let value = match source {
        VersionSource::Json { pointer, .. } => {
            let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
                format!("version source {} is not JSON: {error}", path.display())
            })?;
            document
                .pointer(pointer)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "version source {} pointer {pointer:?} is not a string",
                        path.display()
                    )
                })?
                .to_string()
        }
        VersionSource::Regex { pattern, .. } => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("version source {} is not UTF-8", path.display()))?;
            let expression = Regex::new(pattern).map_err(|error| error.to_string())?;
            let mut captures = expression.captures_iter(text);
            let first = captures
                .next()
                .and_then(|capture| capture.name("version"))
                .map(|value| value.as_str().to_string())
                .ok_or_else(|| {
                    format!("version source {} did not produce version", path.display())
                })?;
            if captures.next().is_some() {
                return Err(format!(
                    "version source {} produced more than one version",
                    path.display()
                ));
            }
            first
        }
        VersionSource::Text { .. } => std::str::from_utf8(&bytes)
            .map_err(|_| format!("version source {} is not UTF-8", path.display()))?
            .trim()
            .to_string(),
    };
    if !identifier(&value) {
        return Err(format!(
            "version source {} produced invalid coordinate {value:?}",
            path.display()
        ));
    }
    Ok(value)
}

pub fn validate_catalog_entry(entry: &ReleaseCatalogEntry) -> Result<(), String> {
    if entry.schema_version != SCHEMA_VERSION
        || !identifier(&entry.product)
        || !sha256(&entry.manifest_sha256)
    {
        return Err("release catalog entry identity is invalid".into());
    }
    validate_product_manifest(&entry.manifest)?;
    let manifest_product = match &entry.manifest {
        ProductManifest::Release(value) => &value.product,
        ProductManifest::NonRelease(value) => &value.product,
    };
    if manifest_product != &entry.product {
        return Err("release catalog product disagrees with its manifest".into());
    }
    if let Some(source) = &entry.source {
        if source.commit.len() != 40
            || !source.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !sha256(&source.source_sha256)
            || source.source_uri
                != format!(
                    "stado://sources/{}/{}/source.tar.gz",
                    entry.product, source.source_sha256
                )
        {
            return Err("release catalog source identity is invalid".into());
        }
    }
    Ok(())
}
