//! The compiled runner-profile declaration and the reads that resolve one row.
//!
//! Runner kinds are data, not commands: the profile named on the command line
//! selects a row here, and the host it installs on is resolved from the
//! canonical registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::deploy::{host_channel, DeployError};
use crate::targets::ComputeTarget;

/// The compiled declaration that makes runner kinds data rather than commands.
pub const DECLARATION_PATH: &str = "stado-rs/data/runner-profiles.json";
const DECLARATION: &str = include_str!("../../../data/runner-profiles.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerProfileDeclaration {
    pub schema_version: u64,
    pub profiles: Vec<RunnerProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerProfile {
    pub name: String,
    pub slug: String,
    pub labels: Vec<String>,
    pub github_runner_group: String,
    pub unit_label: String,
    pub installers: BTreeMap<String, String>,
    pub secrets: Vec<String>,
    pub accepts_repository_scope: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_id_account_item: Option<String>,
}

impl RunnerProfile {
    pub(crate) fn labels_text(&self) -> String {
        self.labels.join(",")
    }

    pub(crate) fn installer_kind(&self, platform: &str) -> Result<&str, DeployError> {
        self.installers
            .get(platform)
            .map(String::as_str)
            .ok_or_else(|| {
                DeployError(format!(
                    "runner profile '{}' declares no installer for '{platform}'; add it to {DECLARATION_PATH}",
                    self.name
                ))
            })
    }

    pub(crate) fn needs_kronika(&self) -> bool {
        self.secrets.iter().any(|secret| secret == "probierz-agent")
    }

    /// Whether this profile's repositories carry the Brama review bearer.
    ///
    /// Keyed on the secret it installs, the way `needs_publisher_bootstrap` is
    /// keyed on `SPARKLE_PRIVATE_KEY`. It used to be keyed on `probierz-agent`,
    /// which is a different declaration — the Kronika signing identity the
    /// runner carries on its own disk — so a profile that wanted the identity
    /// and not the repository secret could not say so, and a repository whose
    /// workflows never call Brama could not get a runner while Brama answered
    /// 502.
    pub(crate) fn needs_model_review(&self) -> bool {
        self.secrets
            .iter()
            .any(|secret| secret == "BRAMA_MODEL_ROUTER_TOKEN")
    }

    pub(crate) fn needs_publisher_bootstrap(&self) -> bool {
        self.secrets
            .iter()
            .any(|secret| secret == "SPARKLE_PRIVATE_KEY")
    }
}

fn parse_declaration() -> Result<RunnerProfileDeclaration, String> {
    let declaration: RunnerProfileDeclaration = serde_json::from_str(DECLARATION)
        .map_err(|error| format!("{DECLARATION_PATH} is invalid: {error}"))?;
    if declaration.schema_version != 1 {
        return Err(format!(
            "{DECLARATION_PATH} schema_version is {}, expected 1",
            declaration.schema_version
        ));
    }
    if declaration.profiles.is_empty() {
        return Err(format!(
            "{DECLARATION_PATH} declares no runner profiles; add at least one profile"
        ));
    }
    let mut names = BTreeSet::new();
    let mut slugs = BTreeSet::new();
    for (index, profile) in declaration.profiles.iter().enumerate() {
        let location = format!("{DECLARATION_PATH}.profiles[{index}]");
        if profile.name.is_empty()
            || !profile
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!(
                "{location}.name must be a lowercase runner profile identifier"
            ));
        }
        if !names.insert(profile.name.as_str()) {
            return Err(format!(
                "{location}.name duplicates runner profile {:?}",
                profile.name
            ));
        }
        if profile.slug.is_empty() || !slugs.insert(profile.slug.as_str()) {
            return Err(format!(
                "{location}.slug must be a non-empty unique runner slug"
            ));
        }
        if profile.labels.is_empty()
            || profile.github_runner_group.is_empty()
            || profile.unit_label.is_empty()
        {
            return Err(format!(
                "{location} must declare labels, github_runner_group, and unit_label"
            ));
        }
        let labels = profile.labels.iter().collect::<BTreeSet<_>>();
        if labels.len() != profile.labels.len() {
            return Err(format!("{location}.labels contains a duplicate label"));
        }
        let secrets = profile.secrets.iter().collect::<BTreeSet<_>>();
        if secrets.len() != profile.secrets.len() {
            return Err(format!("{location}.secrets contains a duplicate secret"));
        }
        if profile.needs_publisher_bootstrap()
            && profile
                .developer_id_account_item
                .as_deref()
                .unwrap_or("")
                .is_empty()
        {
            return Err(format!(
                "{} declares no developer_id_account_item; add it to {DECLARATION_PATH}",
                profile.name
            ));
        }
        for platform in ["darwin-arm64", "linux-amd64"] {
            let kind = profile.installers.get(platform).ok_or_else(|| {
                format!(
                    "{location}.installers declares no {platform}; add it to {DECLARATION_PATH}"
                )
            })?;
            let expected_suffix = if platform == "darwin-arm64" {
                "-launchd"
            } else {
                "-systemd"
            };
            if !kind.ends_with(expected_suffix) {
                return Err(format!(
                    "{location}.installers.{platform} is {kind:?}, expected an installer ending in {expected_suffix:?}"
                ));
            }
        }
    }
    Ok(declaration)
}

static RUNNER_PROFILES: LazyLock<Result<RunnerProfileDeclaration, String>> =
    LazyLock::new(parse_declaration);

pub fn runner_declaration() -> Result<&'static RunnerProfileDeclaration, DeployError> {
    RUNNER_PROFILES
        .as_ref()
        .map_err(|error| DeployError(error.clone()))
}

pub fn runner_profile(name: &str) -> Result<&'static RunnerProfile, DeployError> {
    runner_declaration()?
        .profiles
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            DeployError(format!(
                "runner profile '{name}' is not declared; add it to {DECLARATION_PATH}"
            ))
        })
}

pub(crate) async fn runner_target(name: &str) -> Result<ComputeTarget, DeployError> {
    match host_channel::canonical_target(name).await {
        Ok(target) => Ok(target),
        Err(error) => {
            let detail = error.to_string();
            if detail.contains("is not in the canonical registry") {
                return Err(DeployError(format!(
                    "{name} declares no host target; add it to the canonical fleet registry"
                )));
            }
            if detail.contains("is not a local host") {
                return Err(DeployError(format!(
                    "{name} declares no local host provider; set its kind to a local host capability in the canonical fleet registry or select a local host target"
                )));
            }
            if detail.contains("has no registry-managed ssh destination and is not this host") {
                return Err(DeployError(format!(
                    "{name} declares no reachable host destination; add a registry-managed ssh destination to the canonical fleet registry or run the command on that host"
                )));
            }
            Err(error)
        }
    }
}
