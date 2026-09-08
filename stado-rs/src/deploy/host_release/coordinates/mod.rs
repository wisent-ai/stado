mod refusals;
mod resolve;

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{MANAGED_VERSIONS_KEY, TREE_DIR};
use crate::deploy::products::{self, Install, Product};
use crate::deploy::DeployError;
use crate::targets::ComputeTarget;
use refusals::{declared_version, release_origin_allowed};

pub use refusals::{is_exact_semver, is_sha256};
pub use resolve::resolve_release_request;

pub(crate) use refusals::loopback_http_origin;

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// What an operator asked for, before any of it has been checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRequest {
    pub binary: String,
    pub version: String,
    pub platform: String,
    /// Commit and digest stated by the canonical immutable release manifest.
    pub source_commit: String,
    pub sha256: String,
    pub archive_name: String,
    pub member: String,
    /// The exact published byte count of `archive_name`, read on this side so
    /// the target never has to discover it.
    pub archive_bytes: u64,
    /// The public Stado origin serving immutable releases.
    pub release_api: String,
    pub dry_run: bool,
    pub reinstall: bool,
}

/// A checked request: every refusal below has already been made, so the
/// remote programs can be built from it without re-deciding anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    /// The declared product this delivery carries out.
    pub product: &'static Product,
    pub version: String,
    pub platform: String,
    pub sha256: String,
    pub source_commit: String,
    pub archive_name: String,
    pub member: String,
    pub archive_bytes: u64,
    pub release_api: String,
    pub declared_version: String,
    pub dry_run: bool,
    pub reinstall: bool,
}

impl ReleasePlan {
    /// The exact immutable archive the host will fetch.
    pub fn release_uri(&self) -> String {
        format!(
            "stado://releases/{}/{}/{}/{}",
            self.product.source.product, self.version, self.platform, self.archive_name
        )
    }

    pub fn archive_name(&self) -> &str {
        &self.archive_name
    }

    /// The versioned staging directory this coordinate owns. Kept after a
    /// delivery rather than pruned: naming the previous version is the only
    /// rollback this command has.
    pub fn staged_dir(&self) -> String {
        format!(
            "$HOME/.stado/releases/{}/{}/{}",
            self.product.name, self.version, self.platform
        )
    }

    /// Where the verified artefact is kept, unchanged, after delivery: the
    /// staged program itself, or the staged tree it was extracted into.
    pub fn staged_path(&self) -> String {
        match &self.product.install {
            Install::Program { .. } => format!("{}/{}", self.staged_dir(), self.product.name),
            Install::Tree { .. } => format!("{}/{TREE_DIR}", self.staged_dir()),
        }
    }

    /// The path an operator (and `host inventory`) reads the installed
    /// version out of: the active program, or the install root of a tree.
    pub fn active_path(&self) -> String {
        match &self.product.install {
            Install::Program { root } => format!("{root}/{}", self.product.name),
            Install::Tree { root, .. } => root.clone(),
        }
    }

    /// The host-local paths this delivery must leave exactly as it found
    /// them, as full paths on the host.
    pub fn preserved_paths(&self) -> Vec<String> {
        self.product.preserved_paths()
    }
}

/// Every refusal this command makes before it touches a host.
///
/// All of them are made here, on the control plane, and none of them depend
/// on anything the host says. A request that cannot be delivered correctly
/// should cost zero ssh connections and change nothing.
pub fn plan(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    self_store: bool,
) -> Result<ReleasePlan, DeployError> {
    let product = products::product(&request.binary)?;
    if !is_exact_semver(&request.version) {
        return Err(DeployError(format!(
            "{:?} is not an exact version; --version takes a semantic version such as 0.5.1, \
             never a channel, an alias or a range. A release coordinate is immutable",
            request.version
        )));
    }
    let platform = products::managed_platform(&request.platform)?;
    product.platform(platform)?;
    if target.release_platform != platform {
        return Err(DeployError(format!(
            "target {:?} declares release_platform {}, not {platform}",
            target.name, target.release_platform
        )));
    }
    if !is_sha256(&request.sha256) {
        return Err(DeployError(
            "the canonical release manifest carries an invalid SHA-256".to_string(),
        ));
    }
    if !matches!(request.source_commit.len(), 40 | 64)
        || !request
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeployError(
            "the canonical release manifest carries an invalid source_commit".to_string(),
        ));
    }
    let safe_member = |value: &str| {
        !value.is_empty()
            && !value.chars().any(char::is_control)
            && Path::new(value)
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    };
    if Path::new(&request.archive_name)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(request.archive_name.as_str())
        || !safe_member(&request.member)
    {
        return Err(DeployError(
            "the canonical release manifest carries an unsafe archive name or member".to_string(),
        ));
    }
    if request
        .release_api
        .bytes()
        .any(|byte| byte.is_ascii_whitespace())
        || !release_origin_allowed(&request.release_api, self_store)
    {
        return Err(DeployError(
            "canonical STADO_API_URL must be a whitespace-free HTTPS URL; loopback HTTP is \
             allowed only when the target is its own release store"
                .to_string(),
        ));
    }
    let Some(declared) = declared_version(target, &product.name) else {
        return Err(DeployError(format!(
            "the registry declares no {} version for target {:?}; declare it under \
             {MANAGED_VERSIONS_KEY} first. Delivery carries out a declaration, it does not \
             stand in for one",
            product.name, target.name
        )));
    };
    if declared != request.version {
        return Err(DeployError(format!(
            "the registry declares {} {declared} for target {:?}, not {}. Change the \
             declaration if that is the intent; delivering against it would make the \
             registry describe a host it no longer describes",
            product.name, target.name, request.version
        )));
    }
    Ok(ReleasePlan {
        product,
        version: request.version.clone(),
        platform: platform.to_string(),
        sha256: request.sha256.clone(),
        source_commit: request.source_commit.clone(),
        archive_name: request.archive_name.clone(),
        member: request.member.clone(),
        archive_bytes: request.archive_bytes,
        release_api: request.release_api.trim_end_matches('/').to_string(),
        declared_version: declared.to_string(),
        dry_run: request.dry_run,
        reinstall: request.reinstall,
    })
}
