//! What the inference section of a registry holds: the engines, models,
//! resources and endpoints a deployment names, and the document that carries
//! all of them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Engine {
    pub name: String,
    pub image: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub repository: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resources {
    pub gpu_mode: String,
    pub gpus: u16,
    pub max_model_len: u64,
    #[serde(default)]
    pub kv_cache_memory_gb: Option<u64>,
    #[serde(default)]
    pub cache_dir: Option<String>,
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
    /// are unrestricted. This exists because on 2026-08-26 the fleet's agent
    /// aliases (`weles/agent/primary`, `wisent-backend/chat/*`) were found
    /// pointing at an erotic-roleplay finetune: nothing in the registry said
    /// what the model was for, so nothing could refuse the binding.
    #[serde(default)]
    pub model_purposes: BTreeMap<String, String>,
    /// Declared purpose per alias, for the aliases whose name does not carry
    /// it. `wisent-backend/chat/primary` is the product's own roleplay chat and
    /// its first segment is the consumer, not a purpose, so the namespace rule
    /// alone cannot express what the operator decided twice: on 2026-08-19 that
    /// this alias must serve Cydonia, and on 2026-08-26 that Cydonia must serve
    /// nothing agentic. Declaring the alias's purpose keeps both — an agent
    /// alias with no entry still falls back to its first segment and is still
    /// refused. Do not populate this or `model_purposes` until every host runs
    /// a release that models it: an older binary ignores the field, judges the
    /// binding by namespace alone, and refuses the whole registry document.
    #[serde(default)]
    pub alias_purposes: BTreeMap<String, String>,
}
