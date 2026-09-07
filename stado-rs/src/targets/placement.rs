use super::*;

/// A state file a placement profile must carry with the services it moves.
/// `required` state that is absent aborts the move: half-migrated state is
/// how a vault ends up on the box that is no longer serving it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementState {
    pub path: String,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One logical service's lifecycle on a placement host.
///
/// Managed units retain the original `unit` / `path` / `kind` representation.
/// A release-controlled service instead carries `controller=release-control`
/// and its exact release product; strict registry validation rejects every
/// mixed or partial shape before this round-tripping model is constructed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementUnit {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// "launchd" | "systemd" for a managed unit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PlacementUnit {
    pub fn release_controlled(&self) -> bool {
        self.controller.as_deref() == Some("release-control") && self.product.is_some()
    }
}

/// A health check that proves a service came up on the host it moved to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementProbe {
    pub service: String,
    pub url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What one host needs in order to run a placement profile's services.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementHost {
    #[serde(default)]
    pub units: BTreeMap<String, PlacementUnit>,
    #[serde(default)]
    pub probes: Vec<PlacementProbe>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A group of services that move between hosts together, with the order they
/// stop and start in and the state that travels with them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementProfile {
    pub name: String,
    #[serde(default)]
    pub services: Vec<String>,
    /// Stop order on the host handing over; `start_order` is deliberately
    /// separate rather than the reverse, because a dependency that must stop
    /// last does not always start first.
    #[serde(default)]
    pub stop_order: Vec<String>,
    #[serde(default)]
    pub start_order: Vec<String>,
    #[serde(default)]
    pub state: Vec<PlacementState>,
    #[serde(default)]
    pub hosts: BTreeMap<String, PlacementHost>,
    /// Routing rules the mover rewrites, kept verbatim: this checkout does
    /// not model an entry's shape, and inventing one would delete the parts
    /// it guessed wrong.
    #[serde(default)]
    pub routing: Vec<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
