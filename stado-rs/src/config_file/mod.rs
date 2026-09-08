//! User-facing configuration file for stado.
//!
//! Port of `stado/config_file.py`. Resolution order for every setting:
//! environment variable wins, then the config file, then the built-in
//! default. The file is plain JSON (no new dependency) and is searched at,
//! in order: $STADO_CONFIG, ./stado.config.json, ~/.config/stado/config.json,
//! ~/.stado/config.json.
//!
//! Structured sections (storage/providers/azure/dashboard/alerts/billing/
//! credentials) are flattened onto the constant names config.rs consumes, so
//! no consumer changes are required to adopt a file-driven deployment.
//!
//! The file is loaded once and cached process-wide (Python `_CACHE`), via
//! [`std::sync::OnceLock`]. A parse failure is NOT cached — the next call
//! retries, mirroring the Python behavior where `_CACHE["loaded"]` stays
//! None after a `ValueError`.
//!
//! The components mirror the seams this file already carried: [`discovery`]
//! holds the search path, the process-wide cache and the loader; [`readers`]
//! holds the dotted and catalogued readers a running process consults; and
//! [`validation`] judges a document an operator is about to deploy. The schema
//! contract, its error type and the starting-point template stay here, so
//! `crate::config_file::<item>` resolves exactly as it did before.

use std::path::PathBuf;

use serde_json::Value;

mod discovery;
mod readers;
mod validation;

pub(crate) use discovery::expand_tilde;
pub use discovery::{config_path, find_config_file, load_config_file};
pub use readers::{field_value, get, resolve, resolve_list};
pub use validation::validate;

/// Root configuration contract written by `stado config init`.
pub const SCHEMA_VERSION: u16 = true as u16;

/// Environment variable naming an explicit config file path.
pub const FILE_ENV: &str = "STADO_CONFIG";
/// Candidate config file locations, searched in order after $STADO_CONFIG.
pub const CANDIDATES: [&str; 3] = [
    "stado.config.json",
    "~/.config/stado/config.json",
    "~/.stado/config.json",
];

/// Error raised for an unreadable / malformed config file (Python
/// `ValueError`).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid stado config file {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("stado config file {0} must contain a JSON object")]
    NotAnObject(PathBuf),
}

/// A commented starting-point config, mirroring Python `template()`.
pub fn template() -> Value {
    let local = crate::capabilities::ProviderId::Local.as_str();
    let disabled =
        crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Compute)
            .filter(|provider| *provider != local)
            .collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "providers": [local],
        "providers_disabled": disabled,
        "credentials": {
            "store": "skarbiec",
            "admin": {
                "consumer": "local-operator",
                "token_file": "~/.stado/local-operator-skarbiec-token"
            }
        },
        "storage": {
            "backend": "local",
            "local": {"path": "~/.stado/local-storage"},
            "backup": {
                "backend": "local",
                "local": {"path": "~/.stado/local-backup"}
            }
        },
        "deployment": {"id": ""},
        "dashboard": {
            "bind": "localhost",
            "trust_https_proxy": false
        }
    })
}
