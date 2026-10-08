//! Deployment-owned bounds for deterministic archive creation.

use crate::capabilities::STORAGE_ARCHIVE_LIMITS_CONFIG;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    env::VarError,
    num::{NonZeroU64, NonZeroUsize},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveLimits {
    pub(crate) entries: NonZeroUsize,
    pub(crate) path_bytes: NonZeroUsize,
    pub(crate) member_bytes: NonZeroU64,
    pub(crate) total_bytes: NonZeroU64,
}

impl ArchiveLimits {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| format!(
            "{} must declare positive whole-number entries, path_bytes, member_bytes and total_bytes: {error}",
            STORAGE_ARCHIVE_LIMITS_CONFIG.path
        ))
    }

    pub(crate) fn read() -> Result<Self, String> {
        let field = &STORAGE_ARCHIVE_LIMITS_CONFIG;
        let value = match std::env::var(field.env) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
                format!("{} cannot be read as {} JSON: {error}", field.env, field.path)
            })?,
            Err(VarError::NotPresent) => crate::config_file::field_value(field).ok_or_else(|| {
                format!("archive packing limits are not declared: set {} with stado config set, or {} with its JSON document", field.path, field.env)
            })?,
            Err(error) => return Err(format!("{} cannot be read: {error}", field.env)),
        };
        Self::parse(value)
    }
}
