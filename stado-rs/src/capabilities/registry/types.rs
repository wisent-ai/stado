//! One runtime variant, and the capability whose variants it sits among.

use serde::Serialize;

use crate::capabilities::catalog::ProviderId;
use crate::capabilities::config::ConfigField;
use crate::capabilities::runtime::{RuntimeAdapter, RuntimeFacet, SelectionMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CapabilityVariant {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub provider: Option<ProviderId>,
    pub implementation: &'static str,
    pub summary: &'static str,
    pub configurable: bool,
    pub constructible: bool,
    #[serde(skip)]
    pub adapter: RuntimeAdapter,
    #[serde(skip)]
    pub config: &'static [ConfigField],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Capability {
    pub kind: RuntimeFacet,
    pub selection: SelectionMode,
    pub summary: &'static str,
    pub variants: &'static [CapabilityVariant],
}
