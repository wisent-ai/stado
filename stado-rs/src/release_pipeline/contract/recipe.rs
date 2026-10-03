//! Per-platform build recipes, the runtime contract, promotion policy and
//! deliveries a product declares in `.wisent-release.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::validate::predicates::{default_extract, default_required};

/// One platform's recipe.
///
/// Unknown keys are kept rather than refused, and the reason is a production
/// failure: the workers that build a release run the binary a host already
/// has, so a manifest field added in the same commit as its reader is read
/// by the OLD contract first. Declaring a new field fails the release on
/// every platform with serde's "unknown field" before a single crate
/// compiles, because the deployed worker denies it. A field a
/// worker does not understand must be ignorable; `stado release submit`
/// refuses typos itself, in the binary the operator is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformRecipe {
    pub runner_platform: String,
    pub quality: Vec<QualityGate>,
    pub build: BuildCommand,
    /// Product journeys executed against this build before publication.
    /// Old manifests remain buildable, but cannot qualify submitted tasks.
    #[serde(default)]
    pub tests: Vec<QualityGate>,
    /// Product surfaces this platform's post-build tests run, installed on
    /// the builder through `stado product install` before the first test. A
    /// journey that drives another product's CLI names that product here,
    /// instead of depending on whatever the builder happens to carry.
    #[serde(default)]
    pub test_products: Vec<TestProduct>,
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
    /// Declared, never inferred. Without it a build compiles for twenty
    /// minutes and dies with `No space left on device (os error 28)` while
    /// rustc writes metadata, on a host above its janitor watermark: the
    /// operator learns the requirement from a linker error inside a long log
    /// instead of from a refusal before the first crate.
    #[serde(default)]
    pub min_free_gb: u64,
    /// Keys this contract does not know, kept so a worker running an older
    /// contract can still build a release whose manifest carries a newer
    /// field. `validate_release_manifest` refuses them, in the binary the
    /// operator submits with.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityGate {
    pub name: String,
    pub argv: Vec<String>,
}

/// One product surface a platform's post-build tests need installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProduct {
    pub product: String,
    pub surface: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct RuntimeContract {
    pub binary: String,
    pub launcher: String,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: String,
    #[serde(default)]
    pub rollback_compatible_with: Vec<String>,
    /// What the running service is allowed to do as its own Skarbiec
    /// consumer (named after the product), in `stado credentials token mint
    /// --capabilities` form: `read:<item>#<field>` for a secret it reads,
    /// `call:brama#<alias>` for a Brama route it calls. Enrollment grants
    /// exactly these on the vault owner and delivers the bearer to every
    /// host the product rolls out to, so a new service needs no hand-minted
    /// token.
    #[serde(default)]
    pub grants: Vec<String>,
    /// The loopback port the service answers on once it is live. With it,
    /// enrollment creates the product's rollout policy when the registry has
    /// none: a blue-green target on the host the service directory places it
    /// on (or the vault owner), with two candidate ports Stado picks itself.
    #[serde(default)]
    pub port: Option<u16>,
    /// The HTTP path that answers 200 when the service is ready; `/healthz`
    /// when absent.
    #[serde(default)]
    pub readiness_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct Delivery {
    pub name: String,
    pub platform: String,
    pub argv: Vec<String>,
    pub required: bool,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
    /// A registry host, an empty string for builder placement, or the product's
    /// destination declaration. Product destinations are frozen per release run.
    #[serde(default)]
    pub target: DeliveryTarget,
    /// Earlier deliveries of the same manifest that must pass before this
    /// one is queued, such as a schema migration before the application
    /// that reads it. Deliveries without it are queued together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeliveryTarget {
    Host(String),
    Registry(ProductDestinations),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductDestinations {
    pub product: String,
}

impl Default for DeliveryTarget {
    fn default() -> Self {
        Self::Host(String::new())
    }
}

impl DeliveryTarget {
    pub fn validate(&self, product: &str) -> Result<(), String> {
        use crate::release_pipeline::validate::predicates::identifier;
        match self {
            Self::Host(host) if host.is_empty() || identifier(host) => Ok(()),
            Self::Registry(declared) if declared.product == product => Ok(()),
            _ => Err("delivery target must name a registry host or its own product's destination declaration".into()),
        }
    }

    pub fn host(&self) -> Result<&str, String> {
        match self {
            Self::Host(host) => Ok(host),
            Self::Registry(declared) => Err(format!(
                "delivery destinations for {} were not resolved into an immutable plan",
                declared.product
            )),
        }
    }
}
