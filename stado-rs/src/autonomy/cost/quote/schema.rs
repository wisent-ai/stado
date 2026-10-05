use serde::de::value::{Error, StrDeserializer};
use serde::Deserialize;

/// PriceBook's serialized purchase-option field, not classification of prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Purchase {
    OnDemand,
    Spot,
}

impl Purchase {
    pub(super) fn decode(value: &str) -> Result<Self, String> {
        Self::deserialize(StrDeserializer::<Error>::new(value)).map_err(|error| error.to_string())
    }
}

/// VirtualMachine.properties.priority, as declared by the Azure Compute API.
/// https://learn.microsoft.com/en-us/rest/api/compute/virtual-machines/get#virtualmachineprioritytypes
#[derive(Debug, Deserialize)]
pub(super) enum AzurePriority {
    Regular,
    Low,
    Spot,
}

/// Instance.scheduling.provisioningModel in the Compute Engine v1 schema.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum GcpProvisioningModel {
    Standard,
    Spot,
}

/// An Azure Retail Prices unitOfMeasure of "1 Hour" and a normalized book
/// "hour" both price one hour. The original quote remains in the receipt.
#[derive(Deserialize)]
pub(super) enum HourUnit {
    #[serde(rename = "hour", alias = "1 Hour")]
    Hour,
}

impl HourUnit {
    pub(super) fn accepts(value: &str) -> bool {
        Self::deserialize(StrDeserializer::<Error>::new(value)).is_ok()
    }
}
