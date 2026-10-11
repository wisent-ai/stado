//! The bounds a request is read within: declared by the deployment, or
//! measured from the memory this host can give a request now.

use std::{env::VarError, num::NonZeroUsize};

use serde::Deserialize;
use serde_json::Value;

use crate::capabilities::DASHBOARD_REQUEST_LIMITS_CONFIG;
use crate::dashboard::{fleet_join, operator_console, DashboardError};

/// Deployment-owned bounds, checked before the listener accepts traffic.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestLimits {
    pub(crate) head_bytes: NonZeroUsize,
    pub(crate) body_bytes: NonZeroUsize,
    pub(crate) registry_import_bytes: NonZeroUsize,
    pub(crate) operator_console: operator_console::Limits,
    pub(crate) fleet_join: fleet_join::Limits,
}

impl RequestLimits {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| {
            format!(
                "{} must declare positive whole-byte head_bytes, body_bytes and registry_import_bytes, and positive operator_console and fleet_join bounds: {error}",
                DASHBOARD_REQUEST_LIMITS_CONFIG.path
            )
        })
    }

    pub(crate) fn read() -> Result<Self, DashboardError> {
        let field = &DASHBOARD_REQUEST_LIMITS_CONFIG;
        let value = match std::env::var(field.env) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
                DashboardError::Refused(format!(
                    "{} cannot be read as {} JSON: {error}",
                    field.env, field.path
                ))
            })?,
            Err(VarError::NotPresent) => match crate::config_file::field_value(field) {
                Some(value) => value,
                None => return Self::measured(),
            },
            Err(error) => {
                return Err(DashboardError::Refused(format!(
                    "{} cannot be read: {error}",
                    field.env
                )))
            }
        };
        Self::parse(value).map_err(DashboardError::Refused)
    }

    /// The bounds of a deployment that declares none: every byte bound is the
    /// memory this host can give a request now, and the argument count is
    /// bounded by the same figure, since every argument takes at least a
    /// byte. A request larger than that cannot be held whatever was declared.
    ///
    /// Requiring a declaration instead stopped the vault owner's object API:
    /// a host with no `dashboard.request_limits` refused to start `serve
    /// --api`, and every Stado command in the fleet answered a gateway error.
    /// The values could not be declared either, because nobody stated them
    /// and the operator is not asked for numbers; a bound read from the host
    /// is the source the store's document reads already use.
    pub(crate) fn measured() -> Result<Self, DashboardError> {
        let reading = crate::providers::local::host_memory::read_host_memory();
        let available = reading
            .available_bytes
            .and_then(|bytes| usize::try_from(bytes).ok())
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| {
                DashboardError::Refused(format!(
                    "API request limits are not declared ({} or {}) and this host's available \
                     memory could not be read to bound requests by it: {reading:?}",
                    DASHBOARD_REQUEST_LIMITS_CONFIG.path, DASHBOARD_REQUEST_LIMITS_CONFIG.env
                ))
            })?;
        Ok(Self {
            head_bytes: available,
            body_bytes: available,
            registry_import_bytes: available,
            operator_console: operator_console::Limits {
                argument_count: available,
                argument_bytes: available,
                input_bytes: available,
                request_bytes: available,
            },
            fleet_join: fleet_join::Limits {
                request_bytes: available,
                field_bytes: available,
            },
        })
    }
}
