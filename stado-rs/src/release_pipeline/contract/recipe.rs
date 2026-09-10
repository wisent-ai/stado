//! Per-platform build recipes, the runtime contract, promotion policy and
//! deliveries a product declares in `.wisent-release.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::validate::predicates::{default_extract, default_required};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRecipe {
    pub runner_platform: String,
    pub quality: Vec<QualityGate>,
    pub build: BuildCommand,
    pub stage: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
    /// Build-time variables whose values are not secret and belong in the
    /// repository: a public origin, a feature flag, a base path.
    ///
    /// `secret_env` is the wrong home for those. It names a Skarbiec item and
    /// field, so a public constant stored there becomes a credential the fleet
    /// grants, syncs and audits for no reason, and the value stops being
    /// reviewable in the diff that changed it. `echo-web` needs
    /// `NEXT_PUBLIC_SITE_URL=https://content.wisent.ai` at build time and
    /// that URL is on the public internet.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_required")]
    pub required: bool,
    /// Free space this platform's build needs on the work volume, in GiB, or
    /// zero where the product declares no requirement.
    ///
    /// Declared, never inferred. On 2026-09-10 the stado 0.20.3 darwin build
    /// compiled for twenty minutes on charless-mac-mini and died with
    /// `No space left on device (os error 28)` while rustc wrote metadata,
    /// with roughly 11 GiB free against an 8 GiB janitor watermark: the
    /// operator learnt the requirement from a linker error inside a 30 KB log
    /// instead of from a refusal before the first crate.
    #[serde(default)]
    pub min_free_gb: u64,
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
    #[serde(default = "default_extract")]
    pub extract: bool,
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
pub struct Delivery {
    pub name: String,
    pub platform: String,
    pub argv: Vec<String>,
    pub required: bool,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
    /// Registry target this delivery must run ON. A delivery that installs
    /// software on a host used to run on whatever builder was live and reach
    /// its target over ssh — and the first host without Remote Login broke
    /// the whole release. Pinned to its target, a delivery installs locally
    /// and no delivery needs a login service at all. Empty keeps the old
    /// builder placement for deliveries that publish elsewhere.
    #[serde(default)]
    pub target: String,
}
