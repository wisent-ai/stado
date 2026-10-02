//! The two shapes `.wisent-release.json` may legally declare, and where the
//! product's version coordinate is read from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::recipe::{Delivery, PlatformRecipe, PromotionPolicy, ReleaseInput, RuntimeContract};

// One manifest is parsed per release operation and the two variants are the two
// shapes a product may legally declare. Boxing the release arm would put an
// allocation between every reader and the fields it came for.
#[allow(clippy::large_enum_variant)]
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
    pub inputs: BTreeMap<String, ReleaseInput>,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

/// What a build worker reads of a release manifest: the product and the
/// platform recipes. A builder runs the Stado its host already has, so the
/// sections it does not act on — deliveries, promotion, inputs, runtime — are
/// kept as values rather than parsed, and a shape they take on in a newer
/// Stado cannot stop the build that carries its reader. The control host
/// reads the whole manifest as [`ReleasePipelineManifest`] when it submits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerManifest {
    pub schema_version: u32,
    pub product: String,
    pub releases: bool,
    pub platforms: BTreeMap<String, PlatformRecipe>,
    #[serde(flatten)]
    pub rest: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VersionSource {
    Json { path: String, pointer: String },
    Regex { path: String, pattern: String },
    Text { path: String },
}

impl VersionSource {
    pub(in crate::release_pipeline) fn path(&self) -> &str {
        match self {
            Self::Json { path, .. } | Self::Regex { path, .. } | Self::Text { path } => path,
        }
    }
}
