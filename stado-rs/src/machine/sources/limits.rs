//! Deployment-owned source staging bounds, shared by admission and readback.

use std::{env::VarError, num::NonZeroU64};

use serde::Deserialize;
use serde_json::Value;

use crate::capabilities::MACHINE_SOURCE_LIMITS_CONFIG;
use crate::machine::MachineError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceLimits {
    pub(crate) archive_bytes: NonZeroU64,
    pub(crate) extracted_bytes: NonZeroU64,
    pub(crate) members: NonZeroU64,
    pub(crate) trailing_bytes: NonZeroU64,
}

impl SourceLimits {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| {
            format!(
                "{} must declare positive whole-number archive_bytes, extracted_bytes, members and trailing_bytes: {error}",
                MACHINE_SOURCE_LIMITS_CONFIG.path
            )
        })
    }

    pub(in crate::machine) fn read() -> Result<Self, MachineError> {
        let invalid = |message| MachineError::new("INVALID_SOURCE_ARCHIVE", message);
        let field = &MACHINE_SOURCE_LIMITS_CONFIG;
        let value = match std::env::var(field.env) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
                invalid(format!(
                    "{} cannot be read as {} JSON: {error}",
                    field.env, field.path
                ))
            })?,
            Err(VarError::NotPresent) => crate::config_file::field_value(field).ok_or_else(|| {
                invalid(format!(
                    "source archive limits are not declared: set {} with stado config set, or {} with its JSON document",
                    field.path, field.env
                ))
            })?,
            Err(error) => {
                return Err(invalid(format!("{} cannot be read: {error}", field.env)))
            }
        };
        Self::parse(value).map_err(invalid)
    }
}
