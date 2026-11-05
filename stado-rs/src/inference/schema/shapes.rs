//! What the inference section of a registry holds: the engines, models,
//! resources and endpoints a deployment names, and the document that carries
//! all of them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Engine {
    pub name: String,
    pub image: String,
    /// How a declared engine (any engine but vLLM) is started and asked:
    /// the operator's own declaration, since Stado assumes no image, video
    /// or speech engine. Absent for vLLM, whose start Stado knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared: Option<DeclaredEngine>,
}

/// What Stado needs to run an engine it does not know by name: the
/// arguments after the image, where the engine reads its bearer, the path
/// that answers when it is ready, and the request that proves it generates.
/// `{model}`, `{revision}`, `{name}`, `{host}` and `{port}` in an argument
/// are replaced with the deployment's values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeclaredEngine {
    pub args: Vec<String>,
    /// The environment variable the engine reads its bearer from.
    pub key_variable: String,
    /// A GET path that answers 2xx with the bearer once the engine serves.
    pub ready_path: String,
    /// A POST path and body that make one generation of the deployment's
    /// kind; a 2xx answer proves it generates.
    pub generate_path: String,
    pub generate_body: serde_json::Value,
}

/// What a deployment generates. A text deployment answers OpenAI
/// chat-completions through vLLM; the others answer what their declared
/// engine answers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Text,
    Image,
    Video,
    Speech,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Video => "video",
            Self::Speech => "speech",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub repository: String,
    pub revision: String,
    /// What the repository and its base models declare as their license on
    /// the model hub, read by `stado inference plan` at this revision; absent
    /// on a deployment planned before the plan read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<crate::inference::license::License>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resources {
    pub gpu_mode: super::GpuMode,
    pub max_model_len: u64,
    #[serde(default)]
    pub kv_cache_memory_gb: Option<u64>,
    #[serde(default)]
    pub cache_dir: Option<String>,
    /// The board the engine runs on, by the driver's UUID (`GPU-…`), chosen
    /// at plan time from the host's measured free boards. None for a
    /// deployment planned before boards were named, which holds every board
    /// of its host, as it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub visibility: String,
    pub port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Deployment {
    pub name: String,
    pub target: String,
    pub desired_state: String,
    /// What it generates; a deployment declared before kinds is text.
    #[serde(default)]
    pub kind: Kind,
    pub engine: Engine,
    pub model: Model,
    pub resources: Resources,
    pub endpoint: Endpoint,
    pub credential_item: String,
    #[serde(default)]
    pub previous: Option<Box<Deployment>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    #[serde(default)]
    pub gateway_target: Option<String>,
    #[serde(default)]
    pub deployments: Vec<Deployment>,
    #[serde(default)]
    pub routes: BTreeMap<String, String>,
    #[serde(default)]
    pub fallbacks: BTreeMap<String, Vec<String>>,
    /// Declared purpose per model repository (for example
    /// `TheDrummer/Cydonia-24B-v4.3` -> `erotic-roleplay`). A model with a
    /// declared purpose may only be selected by an alias whose first segment
    /// is that purpose, as a route or as a fallback. Models without an entry
    /// are unrestricted. This exists because agent aliases can otherwise be
    /// found pointing at a roleplay finetune: nothing in the registry says
    /// what the model is for, so nothing can refuse the binding.
    #[serde(default)]
    pub model_purposes: BTreeMap<String, String>,
    /// Declared purpose per alias, for the aliases whose name does not carry
    /// it. `wisent-backend/chat/primary` is the product's own roleplay chat and
    /// its first segment is the consumer, not a purpose, so the namespace rule
    /// alone cannot express two decisions at once: that this alias must serve
    /// a given model, and that the model must serve nothing agentic.
    /// Declaring the alias's purpose keeps both — an agent
    /// alias with no entry still falls back to its first segment and is still
    /// refused. Do not populate this or `model_purposes` until every host runs
    /// a release that models it: an older binary ignores the field, judges the
    /// binding by namespace alone, and refuses the whole registry document.
    #[serde(default)]
    pub alias_purposes: BTreeMap<String, String>,
}
