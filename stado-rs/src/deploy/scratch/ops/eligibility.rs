//! Which hosts a lease may be taken on, answered from the declarations.
//!
//! A caller — an operator at a terminal, or a test that has to reach a real
//! machine — must not choose the host by hardcoding a name or reading an
//! environment variable. Both are a second source of truth about the fleet, and
//! the fleet already has one. This asks it: a target is leasable when the
//! registry declares it local with an ssh destination, and some scratch profile
//! declares the platform that target says it runs.
//!
//! The answer carries the ineligible targets too, with the reason each one is
//! out, because "no host is leasable" and "this host is leasable and the
//! command still failed" send an operator to different places.

use serde_json::{json, Value};

use crate::deploy::scratch::declaration;
use crate::deploy::{host_channel, DeployError};
use crate::targets::ComputeTarget;

/// One target, and whether a lease can be taken on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEligibility {
    pub target: String,
    pub release_platform: String,
    pub ssh: Option<String>,
    /// The profile that covers this target's platform, when one does.
    pub profile: Option<String>,
    /// Why this target is not leasable, in the operator's words.
    pub refusal: Option<String>,
}

impl HostEligibility {
    pub fn eligible(&self) -> bool {
        self.refusal.is_none()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "release_platform": self.release_platform,
            "ssh": self.ssh,
            "profile": self.profile,
            "eligible": self.eligible(),
            "refusal": self.refusal,
        })
    }
}

/// Every registry target, in the registry's own order, with its verdict.
pub async fn eligible_hosts() -> Result<Vec<HostEligibility>, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let declared = declaration::declaration()?;
    Ok(registry
        .targets
        .iter()
        .map(|target| verdict(target, &declared))
        .collect())
}

fn verdict(target: &ComputeTarget, declared: &declaration::ScratchDeclaration) -> HostEligibility {
    let ssh = target
        .ssh_connections()
        .next()
        .map(|(_, destination)| destination.to_string());
    let profile = declared
        .profiles
        .iter()
        .find(|profile| profile.accepts_platform(&target.release_platform))
        .map(|profile| profile.name.clone());
    let refusal = if !crate::capabilities::ProviderId::Local.matches(&target.kind) {
        Some(format!(
            "kind is '{}'; a lease is only taken on a local host",
            target.kind
        ))
    } else if ssh.is_none() {
        Some("no registry-managed ssh destination".to_string())
    } else if profile.is_none() {
        let platforms = declared
            .profiles
            .iter()
            .flat_map(|profile| profile.platforms.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        Some(if target.release_platform.is_empty() {
            format!("declares no release_platform; profiles cover {platforms}")
        } else {
            format!(
                "release_platform '{}' is covered by no profile; profiles cover {platforms}",
                target.release_platform
            )
        })
    } else {
        None
    };
    HostEligibility {
        target: target.name.clone(),
        release_platform: target.release_platform.clone(),
        ssh,
        profile,
        refusal,
    }
}
