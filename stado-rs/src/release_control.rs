//! Signed immutable product releases and registry-owned rollout policy.
//!
//! A release is built once, addressed by exact product/version/platform
//! coordinates, and signed over a canonical manifest. Promotion changes only
//! registry desired state; it never rebuilds or rewrites candidate bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::release::canonical_coordinate;

pub const RELEASE_CONTROL_KEY: &str = "release_control";
pub const RELEASE_MANIFEST_NAME: &str = "release.json";
pub const RELEASE_SIGNATURE_NAME: &str = "release.sig";
pub const RELEASE_ARCHIVE_NAME: &str = "release.tar.gz";
pub const RELEASE_QUALIFICATION_NAME: &str = "qualification.json";
const MAX_RELEASE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub source_revision: String,
    pub source_sha256: String,
    pub pipeline_manifest_sha256: String,
    pub qualification_receipt_sha256: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub binary: String,
    pub launcher: String,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: String,
    #[serde(default)]
    pub rollback_compatible_with: Vec<String>,
    pub qualification: ReleaseQualification,
    pub key_id: String,
    pub built_at: String,
    pub builder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseQualification {
    pub status: QualificationStatus,
    #[serde(default)]
    pub evidence_sha256: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationStatus {
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseControl {
    pub schema_version: u64,
    pub generation: u64,
    pub trusted_keys: BTreeMap<String, String>,
    pub products: BTreeMap<String, ProductReleasePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductReleasePolicy {
    pub service: String,
    pub config_schema: u64,
    pub state_schema: u64,
    pub install_root: String,
    pub binary: String,
    pub launcher: String,
    pub binary_env: String,
    pub port_env: String,
    pub runtime_env: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub signing_key_item: String,
    #[serde(default)]
    pub signing_key_id: String,
    pub strategy: RolloutStrategy,
    pub targets: BTreeMap<String, ReleaseTargetPolicy>,
    #[serde(default)]
    pub desired: Option<DesiredRelease>,
    #[serde(default)]
    pub previous: Option<DesiredRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTargetPolicy {
    pub platform: String,
    pub run_as_user: String,
    pub home: String,
    pub state_dir: String,
    pub runtime_root: String,
    pub logs_root: String,
    /// Blue-green serving coordinates. Their presence is decided by the
    /// policy's strategy, not by the target: a `blue-green` rollout cannot
    /// run without them and a `replace` rollout has no stable bind, no
    /// candidate port pair and nothing HTTP to readiness-check, so
    /// [`validate_registry_contract`] requires them on the one and forbids
    /// them on the other. Declared-that-it-cannot-exist, not
    /// declared-empty: a `replace` target carrying any of them is a policy
    /// that misunderstands what it rolls out.
    #[serde(default)]
    pub stable_bind: Option<String>,
    #[serde(default)]
    pub candidate_ports: Option<[u16; 2]>,
    #[serde(default)]
    pub readiness_path: Option<String>,
    #[serde(default)]
    pub legacy_launchd_label: Option<String>,
    #[serde(default)]
    pub legacy_launchd_plist: Option<String>,
}

/// The serving coordinates a blue-green rollout binds, probes and switches.
/// [`validate_registry_contract`] guarantees all three are present on a
/// `blue-green` target and absent on a `replace` one; consumers ask for this
/// view rather than re-checking the invariant themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueGreenServing {
    pub stable_bind: String,
    pub candidate_ports: [u16; 2],
    pub readiness_path: String,
}

impl ReleaseTargetPolicy {
    /// The blue-green serving coordinates, or why this target has none.
    pub fn blue_green_serving(&self) -> Result<BlueGreenServing, String> {
        match (
            &self.stable_bind,
            &self.candidate_ports,
            &self.readiness_path,
        ) {
            (Some(stable_bind), Some(candidate_ports), Some(readiness_path)) => {
                Ok(BlueGreenServing {
                    stable_bind: stable_bind.clone(),
                    candidate_ports: *candidate_ports,
                    readiness_path: readiness_path.clone(),
                })
            }
            _ => Err(
                "blue-green target must declare stable_bind, candidate_ports and readiness_path"
                    .to_string(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutStrategy {
    pub kind: StrategyKind,
    pub readiness_timeout_seconds: u64,
    pub drain_timeout_seconds: u64,
    pub rollback_window_seconds: u64,
    pub automatic_rollback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrategyKind {
    BlueGreen,
    /// The host-release path swaps the artefact tree in place: there is no
    /// stable proxy bind, no candidate port pair and no HTTP readiness, so
    /// the blue-green release agent must never drive it. The policy exists to
    /// gate who may submit and which version may be promoted.
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredRelease {
    pub version: String,
    pub channel: ReleaseChannel,
    pub rollout_generation: u64,
    pub promoted_at: String,
    pub artifacts: BTreeMap<String, ReleaseArtifactRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Candidate,
    Stable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArtifactRef {
    pub manifest_uri: String,
    pub signature_uri: String,
    pub archive_uri: String,
    pub manifest_sha256: String,
    pub artifact_sha256: String,
    pub source_revision: String,
    pub key_id: String,
}

pub fn control(document: &Value) -> Result<Option<ReleaseControl>, String> {
    document
        .get(RELEASE_CONTROL_KEY)
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("registry.{RELEASE_CONTROL_KEY}: {error}"))
        })
        .transpose()
}

pub fn canonical_manifest(manifest: &ReleaseManifest) -> Result<Vec<u8>, String> {
    serde_json::to_vec(manifest).map_err(|error| format!("cannot encode release manifest: {error}"))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<(u64, String), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("cannot open release artifact {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat release artifact {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RELEASE_BYTES {
        return Err(format!(
            "release artifact must be a non-empty regular file no larger than {MAX_RELEASE_BYTES} bytes"
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read release artifact {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((metadata.len(), hex::encode(digest.finalize())))
}

pub fn generate_signing_key() -> Result<(Vec<u8>, Vec<u8>), String> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "could not generate Ed25519 release signing key".to_string())?;
    let key = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| "generated Ed25519 release signing key is invalid".to_string())?;
    Ok((
        document.as_ref().to_vec(),
        key.public_key().as_ref().to_vec(),
    ))
}

pub fn signing_public_key(private_pkcs8: &[u8]) -> Result<Vec<u8>, String> {
    let key = Ed25519KeyPair::from_pkcs8(private_pkcs8)
        .map_err(|_| "release signing key is not Ed25519 PKCS#8".to_string())?;
    Ok(key.public_key().as_ref().to_vec())
}

pub fn sign_manifest(private_pkcs8: &[u8], manifest: &ReleaseManifest) -> Result<String, String> {
    let key = Ed25519KeyPair::from_pkcs8(private_pkcs8)
        .map_err(|_| "release signing key is not Ed25519 PKCS#8".to_string())?;
    Ok(BASE64.encode(key.sign(&canonical_manifest(manifest)?).as_ref()))
}

pub fn verify_manifest(
    public_key_b64: &str,
    manifest: &ReleaseManifest,
    signature_b64: &str,
) -> Result<(), String> {
    let public_key = BASE64
        .decode(public_key_b64)
        .map_err(|_| "trusted release public key is not base64".to_string())?;
    let signature = BASE64
        .decode(signature_b64.trim())
        .map_err(|_| "release signature is not base64".to_string())?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&canonical_manifest(manifest)?, &signature)
        .map_err(|_| "release manifest signature verification failed".to_string())
}

fn identifier(value: &str) -> bool {
    canonical_coordinate(value)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn safe_absolute(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        && !value.chars().any(char::is_control)
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn validate_manifest(manifest: &ReleaseManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err("release manifest schema_version must be 1".to_string());
    }
    for (name, value) in [
        ("product", manifest.product.as_str()),
        ("version", manifest.version.as_str()),
        ("platform", manifest.platform.as_str()),
        ("key_id", manifest.key_id.as_str()),
    ] {
        if !identifier(value) {
            return Err(format!(
                "release manifest {name} is not a canonical coordinate"
            ));
        }
    }
    if manifest.source_revision.len() != 40
        || !manifest
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "release manifest source_revision must be a full lowercase Git commit".to_string(),
        );
    }
    for (name, value) in [
        ("artifact_sha256", manifest.artifact_sha256.as_str()),
        ("source_sha256", manifest.source_sha256.as_str()),
        (
            "pipeline_manifest_sha256",
            manifest.pipeline_manifest_sha256.as_str(),
        ),
        (
            "qualification_receipt_sha256",
            manifest.qualification_receipt_sha256.as_str(),
        ),
    ] {
        if !sha256(value) {
            return Err(format!(
                "release manifest {name} must be 64 lowercase hexadecimal characters"
            ));
        }
    }
    if manifest.artifact_bytes == 0 || manifest.artifact_bytes > MAX_RELEASE_BYTES {
        return Err("release manifest artifact_bytes is outside the supported range".to_string());
    }
    let runtime_present = !manifest.binary.is_empty()
        || !manifest.launcher.is_empty()
        || manifest.config_schema != 0
        || manifest.state_schema != 0
        || !manifest.minimum_stado_version.is_empty()
        || !manifest.rollback_compatible_with.is_empty();
    if runtime_present
        && (!safe_relative(&manifest.binary)
            || !safe_relative(&manifest.launcher)
            || manifest.config_schema == 0
            || manifest.state_schema == 0
            || !canonical_coordinate(&manifest.minimum_stado_version))
    {
        return Err("release manifest runtime fields are incomplete or invalid".to_string());
    }
    let mut rollback = BTreeSet::new();
    for version in &manifest.rollback_compatible_with {
        if !canonical_coordinate(version) || !rollback.insert(version) {
            return Err(
                "release manifest rollback_compatible_with is invalid or duplicated".to_string(),
            );
        }
    }
    match manifest.qualification.status {
        QualificationStatus::Passed => {
            if !manifest
                .qualification
                .evidence_sha256
                .as_deref()
                .is_some_and(sha256)
                || manifest
                    .qualification
                    .completed_at
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
            {
                return Err(
                    "passed release qualification requires evidence_sha256 and completed_at"
                        .to_string(),
                );
            }
        }
        QualificationStatus::Pending | QualificationStatus::Failed => {}
    }
    Ok(())
}

pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(control) = control(document)? else {
        return Ok(());
    };
    if control.schema_version != 1 {
        return Err("registry.release_control.schema_version must be 1".to_string());
    }
    if control.generation == 0 {
        return Err("registry.release_control.generation must be positive".to_string());
    }
    if control.products.is_empty() {
        return Err("registry.release_control.products must not be empty".to_string());
    }
    for (key_id, public_key) in &control.trusted_keys {
        if !identifier(key_id)
            || BASE64
                .decode(public_key)
                .ok()
                .filter(|bytes| bytes.len() == 32)
                .is_none()
        {
            return Err(format!(
                "registry.release_control.trusted_keys.{key_id} is not an Ed25519 public key"
            ));
        }
    }
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets must be an array".to_string())?;
    let target_names: BTreeSet<_> = targets
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .collect();
    let services = crate::service_resolution::directory(document)?
        .map(|directory| directory.services)
        .unwrap_or_default();
    for (product, policy) in &control.products {
        let location = format!("registry.release_control.products.{product}");
        if !identifier(product) || !services.contains_key(&policy.service) {
            return Err(format!("{location}: product or logical service is invalid"));
        }
        if policy.config_schema == 0
            || policy.state_schema == 0
            || !safe_absolute(&policy.install_root)
            || !safe_relative(&policy.binary)
            || !safe_relative(&policy.launcher)
            || !env_name(&policy.binary_env)
            || !env_name(&policy.port_env)
            || !env_name(&policy.runtime_env)
        {
            return Err(format!(
                "{location}: paths or environment variable names are invalid"
            ));
        }
        if policy.targets.is_empty() {
            return Err(format!("{location}.targets must not be empty"));
        }
        if policy.strategy.readiness_timeout_seconds == 0
            || policy.strategy.drain_timeout_seconds == 0
            || policy.strategy.rollback_window_seconds < policy.strategy.drain_timeout_seconds
        {
            return Err(format!("{location}.strategy has invalid time bounds"));
        }
        for (name, value) in &policy.environment {
            if !env_name(name) || value.is_empty() || value.chars().any(char::is_control) {
                return Err(format!("{location}.environment.{name} is invalid"));
            }
        }
        let mut platforms = BTreeSet::new();
        for (target, target_policy) in &policy.targets {
            if !target_names.contains(target.as_str()) {
                return Err(format!("{location}.targets.{target}: unknown target"));
            }
            // The strategy decides the serving coordinates, not the target: a
            // blue-green rollout cannot run without them and a replace rollout
            // cannot have them. Both directions are errors, so a policy can
            // neither forget the ports it switches between nor invent a bind
            // for a product that serves no HTTP.
            let serving = match policy.strategy.kind {
                StrategyKind::BlueGreen => Some(target_policy.blue_green_serving().map_err(
                    |_| {
                        format!(
                            "{location}.targets.{target}: blue-green rollout requires stable_bind, candidate_ports and readiness_path"
                        )
                    },
                )?),
                StrategyKind::Replace => {
                    if target_policy.stable_bind.is_some()
                        || target_policy.candidate_ports.is_some()
                        || target_policy.readiness_path.is_some()
                    {
                        return Err(format!(
                            "{location}.targets.{target}: replace rollout forbids stable_bind, candidate_ports and readiness_path"
                        ));
                    }
                    None
                }
            };
            if !identifier(&target_policy.platform)
                || !identifier(&target_policy.run_as_user)
                || !safe_absolute(&target_policy.home)
                || !safe_absolute(&target_policy.state_dir)
                || !safe_absolute(&target_policy.runtime_root)
                || !safe_absolute(&target_policy.logs_root)
                || target_policy
                    .legacy_launchd_plist
                    .as_deref()
                    .is_some_and(|path| !safe_absolute(path))
                || serving.as_ref().is_some_and(|serving| {
                    !serving.readiness_path.starts_with('/')
                        || serving.readiness_path.contains("..")
                })
            {
                return Err(format!("{location}.targets.{target}: invalid platform, identity, path, or readiness path"));
            }
            if let Some(serving) = &serving {
                let bind: SocketAddr = serving.stable_bind.parse().map_err(|_| {
                    format!("{location}.targets.{target}.stable_bind is not a socket address")
                })?;
                if !bind.ip().is_loopback()
                    || serving.candidate_ports[0] == serving.candidate_ports[1]
                    || serving.candidate_ports.contains(&bind.port())
                {
                    return Err(format!(
                        "{location}.targets.{target}: blue-green ports are invalid"
                    ));
                }
            }
            platforms.insert(target_policy.platform.clone());
        }
        for desired in [policy.desired.as_ref(), policy.previous.as_ref()]
            .into_iter()
            .flatten()
        {
            if !canonical_coordinate(&desired.version) || desired.rollout_generation == 0 {
                return Err(format!(
                    "{location}.desired release coordinate or generation is invalid"
                ));
            }
            for platform in &platforms {
                let artifact = desired.artifacts.get(platform).ok_or_else(|| {
                    format!("{location}.desired.artifacts: missing platform {platform}")
                })?;
                for uri in [
                    &artifact.manifest_uri,
                    &artifact.signature_uri,
                    &artifact.archive_uri,
                ] {
                    if !uri.starts_with(&format!(
                        "stado://releases/{product}/{}/{platform}/",
                        desired.version
                    )) {
                        return Err(format!(
                            "{location}.desired artifact URI is outside its immutable coordinate"
                        ));
                    }
                }
                if !sha256(&artifact.manifest_sha256)
                    || !sha256(&artifact.artifact_sha256)
                    || !control.trusted_keys.contains_key(&artifact.key_id)
                {
                    return Err(format!(
                        "{location}.desired artifact trust metadata is invalid"
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn release_base(product: &str, version: &str, platform: &str) -> Result<String, String> {
    if !identifier(product) || !identifier(version) || !identifier(platform) {
        return Err(
            "release product, version, and platform must be canonical coordinates".to_string(),
        );
    }
    Ok(format!("stado://releases/{product}/{version}/{platform}"))
}

pub fn safe_extract_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_RELEASE_BYTES {
        return Err("release archive size is outside the supported range".to_string());
    }
    if destination.exists() {
        return Err(format!(
            "immutable release directory already exists: {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "release destination has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create release parent {}: {error}", parent.display()))?;
    let staging = parent.join(format!(".release-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&staging).map_err(|error| {
        format!(
            "cannot create release staging {}: {error}",
            staging.display()
        )
    })?;
    let result = (|| {
        let decoder = flate2::read::GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|error| format!("cannot read release archive: {error}"))?;
        let mut count = 0_usize;
        let mut extracted_bytes = 0_u64;
        for entry in entries {
            count += 1;
            if count > MAX_ARCHIVE_ENTRIES {
                return Err(format!(
                    "release archive exceeds {MAX_ARCHIVE_ENTRIES} entries"
                ));
            }
            let mut entry = entry.map_err(|error| format!("cannot read release entry: {error}"))?;
            let archived_path = entry
                .path()
                .map_err(|error| format!("invalid release entry path: {error}"))?
                .into_owned();
            let mut path = PathBuf::new();
            for component in archived_path.components() {
                match component {
                    Component::Normal(segment) => path.push(segment),
                    Component::CurDir => {}
                    Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                        return Err(format!(
                            "unsafe release entry path: {}",
                            archived_path.display()
                        ));
                    }
                }
            }
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                continue;
            }
            if path.as_os_str().is_empty() {
                if kind.is_dir() {
                    continue;
                }
                return Err("release archive contains an empty file path".to_string());
            }
            if !kind.is_file() && !kind.is_dir() {
                return Err(format!(
                    "release entry is not a regular file or directory: {}",
                    path.display()
                ));
            }
            if kind.is_file() {
                extracted_bytes = extracted_bytes
                    .checked_add(entry.header().size().map_err(|error| {
                        format!("invalid release entry size for {}: {error}", path.display())
                    })?)
                    .ok_or_else(|| "release archive expanded size overflowed".to_string())?;
                if extracted_bytes > MAX_EXTRACTED_BYTES {
                    return Err(format!(
                        "release archive expands beyond {MAX_EXTRACTED_BYTES} bytes"
                    ));
                }
            }
            let output = staging.join(&path);
            if kind.is_dir() {
                std::fs::create_dir_all(&output).map_err(|error| {
                    format!(
                        "cannot create release directory {}: {error}",
                        output.display()
                    )
                })?;
                continue;
            }
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "cannot create release directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| {
                    format!("cannot create release file {}: {error}", output.display())
                })?;
            std::io::copy(&mut entry, &mut file).map_err(|error| {
                format!("cannot extract release file {}: {error}", output.display())
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let source_mode = entry.header().mode().unwrap_or(0);
                let executable = source_mode & 0o111 != 0;
                let owner_only = source_mode & 0o077 == 0;
                let mode = match (executable, owner_only) {
                    (true, true) => 0o700,
                    (true, false) => 0o755,
                    (false, _) => 0o600,
                };
                std::fs::set_permissions(&output, std::fs::Permissions::from_mode(mode)).map_err(
                    |error| format!("cannot set release mode {}: {error}", output.display()),
                )?;
            }
        }
        if count == 0 {
            return Err("release archive is empty".to_string());
        }
        std::fs::rename(&staging, destination).map_err(|error| {
            format!(
                "cannot commit immutable release {} -> {}: {error}",
                staging.display(),
                destination.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

pub fn install_directory(
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    manifest: &ReleaseManifest,
) -> PathBuf {
    Path::new(&target.home)
        .join(&policy.install_root)
        .join("releases")
        .join(&manifest.version)
        .join(&manifest.platform)
}

#[cfg(test)]
mod tests {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    fn valid_manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: 1,
            product: "brama".to_string(),
            version: "0.2.2".to_string(),
            platform: "darwin-arm64".to_string(),
            source_revision: "151ce0f907cc1e7b22a2c4e7356a4251444f4d42".to_string(),
            source_sha256: "714861bf4a2001654d271a62ac1ed99f3f0ce302142baad608b32f13ce056792"
                .to_string(),
            pipeline_manifest_sha256:
                "d460e56bf4bf1582f4c63c31eb8219e8e6bd0b3492d7d0ed59cb2d835326c1d0".to_string(),
            qualification_receipt_sha256:
                "ac891e0e5036507e4eb28e5f6652e319de6e3f9d2be895f388a931a51b596081".to_string(),
            artifact_sha256: "119f93dd06634e9249eef8ae633d2bc02139c588f19fe05f1c7864224182c9ef"
                .to_string(),
            artifact_bytes: 5_000_115,
            binary: "bin/brama".to_string(),
            launcher: "bin/start-with-skarbiec".to_string(),
            config_schema: 1,
            state_schema: 1,
            minimum_stado_version: "0.5.1".to_string(),
            rollback_compatible_with: Vec::new(),
            qualification: ReleaseQualification {
                status: QualificationStatus::Passed,
                evidence_sha256: Some(
                    "d50861c5b162c1d55c0479a47db34e56019211d9580ca621adf22919631f9b01".to_string(),
                ),
                completed_at: Some("2026-08-05T04:00:00Z".to_string()),
            },
            key_id: "brama-release-2026-08".to_string(),
            built_at: "2026-08-05T02:42:52Z".to_string(),
            builder: "github-actions/30970402593/1".to_string(),
        }
    }

    fn archive() -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents, mode) in [
            ("bin/brama", b"release-binary".as_slice(), 0o755),
            ("etc/brama-skarbiec/trust.json", b"{}".as_slice(), 0o600),
            (
                "etc/brama-skarbiec/worm-receipt",
                b"#!/bin/sh\nprintf receipt\n".as_slice(),
                0o700,
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(contents.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            builder.append(&header, contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn manifest_signature_rejects_tampering() {
        let manifest = valid_manifest();
        let (private, public) = generate_signing_key().unwrap();
        let signature = sign_manifest(&private, &manifest).unwrap();

        verify_manifest(&BASE64.encode(public), &manifest, &signature).unwrap();

        let mut tampered = manifest;
        tampered.version = "0.2.3".to_string();
        assert!(verify_manifest(
            &BASE64.encode(signing_public_key(&private).unwrap()),
            &tampered,
            &signature
        )
        .is_err());
    }

    #[test]
    fn passed_qualification_requires_evidence_and_completion_time() {
        let mut manifest = valid_manifest();
        validate_manifest(&manifest).unwrap();

        manifest.qualification.evidence_sha256 = None;
        assert_eq!(
            validate_manifest(&manifest).unwrap_err(),
            "passed release qualification requires evidence_sha256 and completed_at"
        );

        manifest.qualification.status = QualificationStatus::Pending;
        validate_manifest(&manifest).unwrap();
    }

    #[test]
    fn archive_extraction_is_bounded_and_immutable() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("release");
        let bytes = archive();

        safe_extract_archive(&bytes, &destination).unwrap();
        assert_eq!(
            std::fs::read(destination.join("bin/brama")).unwrap(),
            b"release-binary"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(destination.join("bin/brama"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
            assert_eq!(
                std::fs::metadata(destination.join("etc/brama-skarbiec/trust.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(destination.join("etc/brama-skarbiec/worm-receipt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(safe_extract_archive(&bytes, &destination).is_err());
        assert!(safe_extract_archive(&[], &temporary.path().join("empty")).is_err());
    }
}
