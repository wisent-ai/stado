//! Deserialized shapes of the placement declarations and runtime records.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementProfile {
    pub name: String,
    pub services: Vec<String>,
    pub stop_order: Vec<String>,
    pub start_order: Vec<String>,
    pub state: Vec<PlacementState>,
    pub hosts: BTreeMap<String, PlacementHost>,
    #[serde(default)]
    pub allow_unhealthy_source: bool,
    #[serde(default)]
    pub routing: Vec<PlacementRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementState {
    /// `$HOME`-relative path. State is never accepted from outside the host home.
    pub path: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementHost {
    /// Logical service name -> concrete unit installed on this host.
    pub units: BTreeMap<String, PlacementUnit>,
    pub probes: Vec<PlacementProbe>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementUnit {
    /// Logical service identity recorded in `targets[].services[]` for a
    /// Stado-managed unit.
    pub name: String,
    /// Exactly one lifecycle shape. Existing managed units keep their
    /// byte-for-byte `unit` / `path` / `kind` shape; release-controlled
    /// services carry only `controller` / `product`.
    pub lifecycle: PlacementLifecycle,
}

impl<'de> Deserialize<'de> for PlacementUnit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = Map::<String, Value>::deserialize(deserializer)?;
        let keys = raw.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let managed_keys = BTreeSet::from(["kind", "name", "path", "unit"]);
        let release_keys = BTreeSet::from(["controller", "name", "product"]);
        let string = |key: &str| {
            raw.get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{key} must be a string"))
        };
        if keys == managed_keys {
            return Ok(Self {
                name: string("name").map_err(serde::de::Error::custom)?,
                lifecycle: PlacementLifecycle::Managed(ManagedPlacementUnit {
                    unit: string("unit").map_err(serde::de::Error::custom)?,
                    path: string("path").map_err(serde::de::Error::custom)?,
                    kind: string("kind").map_err(serde::de::Error::custom)?,
                }),
            });
        }
        if keys == release_keys {
            let controller = string("controller").map_err(serde::de::Error::custom)?;
            if controller != "release-control" {
                return Err(serde::de::Error::custom(
                    "controller must be exactly \"release-control\"",
                ));
            }
            return Ok(Self {
                name: string("name").map_err(serde::de::Error::custom)?,
                lifecycle: PlacementLifecycle::ReleaseControl(ReleaseControlledPlacementUnit {
                    controller: ReleaseController::ReleaseControl,
                    product: string("product").map_err(serde::de::Error::custom)?,
                }),
            });
        }
        Err(serde::de::Error::custom(
            "must contain exactly managed fields [name, unit, path, kind] or release-controlled fields [name, controller, product]",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PlacementLifecycle {
    Managed(ManagedPlacementUnit),
    ReleaseControl(ReleaseControlledPlacementUnit),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedPlacementUnit {
    /// launchd label or systemd unit name.
    pub unit: String,
    /// Absolute unit-file path on the target host.
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseControlledPlacementUnit {
    pub controller: ReleaseController,
    pub product: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ReleaseController {
    #[serde(rename = "release-control")]
    ReleaseControl,
}

impl PlacementUnit {
    pub fn managed(&self) -> Option<&ManagedPlacementUnit> {
        match &self.lifecycle {
            PlacementLifecycle::Managed(unit) => Some(unit),
            PlacementLifecycle::ReleaseControl(_) => None,
        }
    }

    pub fn release_controlled(&self) -> Option<&ReleaseControlledPlacementUnit> {
        match &self.lifecycle {
            PlacementLifecycle::Managed(_) => None,
            PlacementLifecycle::ReleaseControl(owner) => Some(owner),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementProbe {
    pub service: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRoute {
    /// Host on which the routing unit is installed.
    pub host: String,
    pub unit: PlacementUnit,
    /// The routing unit is enabled only while this is the selected destination.
    pub active_when_destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementTransaction {
    pub id: String,
    pub profile: String,
    pub from_host: String,
    pub to_host: String,
    pub started_at: String,
}
