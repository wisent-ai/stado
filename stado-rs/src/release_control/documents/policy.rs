//! Registry-owned rollout policy: the trusted keys, the per-product rollout
//! strategy and targets, and the desired release those targets converge on.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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

pub(in crate::release_control) fn valid_legacy_launchd_unit(label: &str, plist: &str) -> bool {
    let path = Path::new(plist);
    !label.is_empty()
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && path.parent() == Some(Path::new("/Library/LaunchDaemons"))
        && path.file_stem().and_then(|name| name.to_str()) == Some(label)
        && path.extension().and_then(|extension| extension.to_str()) == Some("plist")
}

/// The serving coordinates a blue-green rollout binds, probes and switches.
/// [`validate_registry_contract`] guarantees all three are present on a
/// `blue-green` target and absent on a `replace` one; consumers ask for this
/// view rather than re-checking the invariant themselves.
///
/// [`validate_registry_contract`]: crate::release_control::validate_registry_contract
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
