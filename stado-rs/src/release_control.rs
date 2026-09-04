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
/// The one object that says which build a coordinate belongs to.
///
/// Immutability protects an object, not a version. Two publishers write this
/// prefix — the tag train writes the executables, `SHA256SUMS`, the platform
/// archive and `release-manifest-<platform>.json`; the signed pipeline writes
/// [`RELEASE_MANIFEST_NAME`], [`RELEASE_SIGNATURE_NAME`],
/// [`RELEASE_ARCHIVE_NAME`] and [`RELEASE_QUALIFICATION_NAME`] — and those two
/// name sets are DISJOINT, so `--if-absent` never refused either of them.
/// A version number lives in `Cargo.toml`, which many commits share, so both
/// producers were entitled to the same coordinate from different revisions.
///
/// `stado/0.13.46/darwin-arm64` is what that costs: `release.json` attests
/// `446ad490…`, `release-manifest-darwin-arm64.json` attests `641a52b2…`, and
/// `pipeline_catalog_identity` refuses to deliver a version that means two
/// builds. It refuses at delivery, after both writes; immutable objects mean
/// the version can never be made to mean one build again.
///
/// This object is claimed create-only BEFORE any artifact, by every publisher,
/// so the second revision is refused while nothing has been written yet.
pub const RELEASE_REVISION_NAME: &str = "source-revision.json";
const MAX_RELEASE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4096;

/// The source revision one `product/version/platform` coordinate attests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinateRevision {
    pub schema_version: u32,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub source_revision: String,
}

impl CoordinateRevision {
    /// One claim for exactly these coordinates and this commit.
    ///
    /// The revision must be a full lowercase Git commit, the same shape
    /// [`validate_manifest`] requires: an abbreviated or uppercase spelling of
    /// one commit would compare unequal to the same commit written the other
    /// way, and this record exists to be compared.
    pub fn new(
        product: &str,
        version: &str,
        platform: &str,
        source_revision: &str,
    ) -> Result<Self, String> {
        if !identifier(product) || !identifier(version) || !identifier(platform) {
            return Err(
                "release product, version, and platform must be canonical coordinates".to_string(),
            );
        }
        if source_revision.len() != 40
            || !source_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(
                "coordinate source revision must be a full lowercase Git commit".to_string(),
            );
        }
        Ok(Self {
            schema_version: 1,
            product: product.to_string(),
            version: version.to_string(),
            platform: platform.to_string(),
            source_revision: source_revision.to_string(),
        })
    }

    /// The exact bytes this claim is stored as.
    ///
    /// Field order is the declaration order and every value is a checked
    /// identifier, so two publishers holding the same facts serialize the same
    /// bytes — which is what makes the create-only put idempotent for a
    /// republish and a refusal for a different build.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| error.to_string())
    }

    /// Whether this claim describes the coordinate the caller is publishing.
    pub fn describes(&self, product: &str, version: &str, platform: &str) -> bool {
        self.schema_version == 1
            && self.product == product
            && self.version == version
            && self.platform == platform
    }
}

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

/// The readiness path a `replace` rollout probes when its target declares
/// none.
///
/// `/healthz` is the path every managed Stado service already answers on, and
/// the one every `readiness_path` in this fleet's registry has ever carried.
/// It is a default rather than a requirement because requiring the key made
/// the registry unwritable by the versions running in the fleet — see the
/// comment at the rollout-target validation.
pub const DEFAULT_REPLACE_READINESS_PATH: &str = "/healthz";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTargetPolicy {
    pub platform: String,
    pub run_as_user: String,
    pub home: String,
    pub state_dir: String,
    pub runtime_root: String,
    pub logs_root: String,
    /// Serving coordinates. Blue-green targets require the stable bind,
    /// candidate ports and readiness path. Replace targets swap one service
    /// tree in place and then prove that exact release through the service's
    /// own HTTP contract, so they MAY omit `readiness_path` and take
    /// [`DEFAULT_REPLACE_READINESS_PATH`]: 0.13.20 and 0.13.23 refuse a
    /// replace target that carries the key at all, so requiring it here left
    /// no document both they and this version accept.
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

fn valid_legacy_launchd_unit(label: &str, plist: &str) -> bool {
    let path = Path::new(plist);
    !label.is_empty()
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && plist.starts_with("/Library/LaunchDaemons/")
        && path.is_absolute()
        && path.file_stem().and_then(|name| name.to_str()) == Some(label)
        && path.extension().and_then(|extension| extension.to_str()) == Some("plist")
}

/// Fill an absent rollout-target legacy unit from the same host's exact
/// service declaration.
///
/// Explicit rollout fields remain the compatibility override. Otherwise only
/// a single service whose `name` equals [`ProductReleasePolicy::service`] can
/// supply the unit; a different name is absence, never a heuristic match.
pub fn hydrate_legacy_launchd_unit(
    document: &Value,
    target_name: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<ReleaseTargetPolicy, String> {
    let mut hydrated = target.clone();
    match (
        target.legacy_launchd_label.as_deref(),
        target.legacy_launchd_plist.as_deref(),
    ) {
        (Some(label), Some(plist)) => {
            if !valid_legacy_launchd_unit(label, plist) {
                return Err(format!(
                    "release target {target_name} declares an invalid legacy launchd label or path"
                ));
            }
            return Ok(hydrated);
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(format!(
                "release target {target_name} declares only part of its legacy launchd unit"
            ));
        }
        (None, None) => {}
    }

    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets must be an array".to_string())?;
    let mut declared_targets = targets
        .iter()
        .filter(|candidate| candidate.get("name").and_then(Value::as_str) == Some(target_name));
    let declared_target = declared_targets
        .next()
        .ok_or_else(|| format!("registry declares no target named {target_name}"))?;
    if declared_targets.next().is_some() {
        return Err(format!(
            "registry declares target {target_name} more than once"
        ));
    }
    let services = declared_target
        .get("services")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut matches = services.iter().filter(|service| {
        service.get("name").and_then(Value::as_str) == Some(policy.service.as_str())
    });
    let Some(service) = matches.next() else {
        return Ok(hydrated);
    };
    if matches.next().is_some() {
        return Err(format!(
            "target {target_name} declares service {} more than once",
            policy.service
        ));
    }
    if service.get("kind").and_then(Value::as_str) != Some("launchd") {
        return Err(format!(
            "target {target_name} service {} is not launchd",
            policy.service
        ));
    }
    let label = service
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let plist = service
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_legacy_launchd_unit(label, plist) {
        return Err(format!(
            "target {target_name} service {} has an invalid launchd label or path",
            policy.service
        ));
    }
    hydrated.legacy_launchd_label = Some(label.to_string());
    hydrated.legacy_launchd_plist = Some(plist.to_string());
    Ok(hydrated)
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
    /// The host-release path swaps the artefact tree in place. It has no stable
    /// proxy bind or candidate port pair, but it still proves the exact release
    /// through the target's declared readiness path.
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

/// A canonical coordinate: ASCII alphanumerics plus `.`, `_` and `-`, no
/// surrounding whitespace, non-empty.
///
/// Crate-visible because the unit-image revisit policy validates launchd
/// labels against exactly this shape, and a second spelling of "what a
/// canonical name may contain" is a second answer waiting to disagree with
/// this one.
pub(crate) fn identifier(value: &str) -> bool {
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

/// An absolute path with no control characters and no `..` component.
///
/// Crate-visible for the same reason [`identifier`] is: the unit-image revisit
/// policy declares its own `state_dir` and holds it to exactly this shape,
/// and a second spelling of "a safe absolute path" is a second answer.
pub(crate) fn safe_absolute(value: &str) -> bool {
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
            // Blue-green and replace share the readiness contract, and only
            // blue-green owns serving coordinates for a second candidate.
            //
            // A replace target may OMIT `readiness_path` and take
            // [`DEFAULT_REPLACE_READINESS_PATH`]. That is not a convenience:
            // requiring the key here made this document unwritable by the
            // fleet that has to obey it. Stado 0.13.20 and 0.13.23 REFUSE a
            // replace target carrying `readiness_path` at all — "replace
            // rollout forbids stable_bind, candidate_ports and
            // readiness_path" — and this validator required it, so on
            // 2026-09-01 no single registry document satisfied both: with the
            // key present the always-on Mac's 0.13.20 queue agent could
            // resolve no policy and stopped scanning its disk for twelve
            // minutes; with it absent every write from the operator's own
            // installed 0.13.26 binary was refused. The fleet could only be
            // written by a build older than the one it was running, and the
            // workaround was to keep that older build around. Validation is
            // whole-document, so one field in one product's rollout froze
            // every domain — instance 16's blast radius with instance 17's
            // version skew.
            //
            // Accepting the absence is the additive shape: it admits both the
            // old constraint and the new one, and any document written for
            // either version validates under both for as long as both exist.
            let readiness_path = match policy.strategy.kind {
                StrategyKind::Replace => target_policy
                    .readiness_path
                    .as_deref()
                    .unwrap_or(DEFAULT_REPLACE_READINESS_PATH),
                StrategyKind::BlueGreen => {
                    target_policy.readiness_path.as_deref().ok_or_else(|| {
                        format!(
                            "{location}.targets.{target}: blue-green rollout requires readiness_path"
                        )
                    })?
                }
            };
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
                    {
                        return Err(format!(
                            "{location}.targets.{target}: replace rollout forbids stable_bind and candidate_ports"
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
                || !readiness_path.starts_with('/')
                || readiness_path.contains("..")
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
    safe_extract_archive_reader(bytes, destination)
}

/// Extract an already-verified archive without reading it back into memory.
/// The exact signed byte count is checked again at the file boundary.
pub fn safe_extract_archive_file(
    archive_path: &Path,
    expected_bytes: u64,
    destination: &Path,
) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|error| {
        format!(
            "cannot open release archive {}: {error}",
            archive_path.display()
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        format!(
            "cannot stat release archive {}: {error}",
            archive_path.display()
        )
    })?;
    if !metadata.is_file()
        || expected_bytes == 0
        || expected_bytes > MAX_RELEASE_BYTES
        || metadata.len() != expected_bytes
    {
        return Err("release archive size differs from its signed manifest".to_string());
    }
    safe_extract_archive_reader(file, destination)
}

fn safe_extract_archive_reader(reader: impl Read, destination: &Path) -> Result<(), String> {
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
        let decoder = flate2::read::GzDecoder::new(reader);
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
