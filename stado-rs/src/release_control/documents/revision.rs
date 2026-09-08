//! The source-revision claims one coordinate and one product version attest.

use serde::{Deserialize, Serialize};

use crate::release_control::identifier;

fn full_git_revision(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

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
    ///
    /// [`validate_manifest`]: crate::release_control::validate_manifest
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
        if !full_git_revision(source_revision) {
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
/// The source revision shared by every platform under one `product/version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRevision {
    pub schema_version: u32,
    pub product: String,
    pub version: String,
    pub source_revision: String,
}

impl VersionRevision {
    pub fn new(product: &str, version: &str, source_revision: &str) -> Result<Self, String> {
        if !identifier(product) || !identifier(version) {
            return Err("release product and version must be canonical coordinates".to_string());
        }
        if !full_git_revision(source_revision) {
            return Err("version source revision must be a full lowercase Git commit".to_string());
        }
        Ok(Self {
            schema_version: 1,
            product: product.to_string(),
            version: version.to_string(),
            source_revision: source_revision.to_string(),
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| error.to_string())
    }

    pub fn describes(&self, product: &str, version: &str) -> bool {
        self.schema_version == 1 && self.product == product && self.version == version
    }
}
