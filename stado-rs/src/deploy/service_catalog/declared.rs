//! Parts of a catalog service declaration that other commands read: the
//! request that proves the service answers, and the stores that travel with
//! it when it moves between hosts.

use serde::Deserialize;

/// How a service is asked whether it answers, and what to do when it does
/// not: the method and path of a request on its own API, the body to send,
/// and the repair steps of its repair declaration to run before the restart.
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AnsweringProbe {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    /// Steps of this service's repair declaration
    /// (`stado-rs/data/catalog/repair-catalog.json`), run in this order
    /// before the restart. One service stands for more than one cause, and
    /// each step reads its own cause and refuses when that cause is absent,
    /// so every step that could clear the standing is declared; a restart
    /// alone ends the process but leaves a wedged daemon it depends on.
    #[serde(default)]
    pub repair: Vec<String>,
}

/// A store under the account's home that a service keeps and that moves with
/// it to another host: a file, or a whole directory (`tree`). `required`
/// says a move refuses when the source has none; a store the service makes
/// on first use is not required. `stado placement profile draft` writes
/// these into the profile it prints, so a moved service finds what it kept.
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogState {
    /// Relative to the account's home.
    pub path: String,
    #[serde(default)]
    pub tree: bool,
    #[serde(default)]
    pub required: bool,
}
