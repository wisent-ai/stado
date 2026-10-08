//! The placement host's certificate policy for fleet PostgreSQL.

use std::{env::VarError, num::NonZeroU32};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::DATABASE_POSTGRES_TLS_CONFIG;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostgresTls {
    pub(crate) certificate_days: NonZeroU32,
    pub(crate) rsa_bits: NonZeroU32,
}

impl PostgresTls {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| {
            format!(
                "{} must declare positive whole-number certificate_days and rsa_bits: {error}",
                DATABASE_POSTGRES_TLS_CONFIG.path
            )
        })
    }

    pub(crate) fn read() -> Result<Self, String> {
        let field = &DATABASE_POSTGRES_TLS_CONFIG;
        let value = match std::env::var(field.env) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
                format!("{} cannot be read as {} JSON: {error}", field.env, field.path)
            })?,
            Err(VarError::NotPresent) => crate::config_file::field_value(field).ok_or_else(|| {
                format!(
                    "fleet PostgreSQL TLS policy is not declared: set {} on the placement host with stado config set, or {} with its JSON document",
                    field.path, field.env
                )
            })?,
            Err(error) => return Err(format!("{} cannot be read: {error}", field.env)),
        };
        Self::parse(value)
    }
}
