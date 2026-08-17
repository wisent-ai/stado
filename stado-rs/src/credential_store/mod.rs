//! Pluggable credential store selected by `STADO_CREDENTIALS_STORE`.
//!
//! The selector is also persisted as `credentials.store` in Stado's config.
//! An environment override that differs from the persisted selector is a
//! pending migration, not an empty new store: normal reads and writes fail
//! closed until `stado secrets migrate` moves every item and commits the new
//! selector.
//!
//! Supported backends:
//! - unset, empty, `skarbiec`, or `skarbiec://<base-url>` — Skarbiec;
//! - `file://<absolute-path>` or a bare absolute path — an owner-only JSON
//!   store, useful for local/offline deployments;
//! - every other scheme — a hard error.
//!
//! All application credential CRUD flows through this module. Backend access
//! grants remain bootstrap credentials outside the selected store: keeping a
//! store's own unlock credential inside that same store would be circular.

use std::path::PathBuf;

use serde_json::Value;

use crate::skarbiec::{Client, SkarbiecError};

pub const ENV_STORE: &str = "STADO_CREDENTIALS_STORE";
const DEFAULT_STORE: &str = "skarbiec";

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod file;
pub mod migrate;
pub mod owner;
#[cfg(test)]
mod tests;
pub mod write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    Skarbiec { url: Option<String> },
    File { path: PathBuf },
}

impl Backend {
    pub(crate) fn locator(&self) -> String {
        match self {
            Self::Skarbiec { url: None } => DEFAULT_STORE.to_string(),
            Self::Skarbiec { url: Some(url) } => format!("skarbiec://{url}"),
            Self::File { path } => format!("file://{}", path.display()),
        }
    }
}
#[derive(Clone, Debug)]
pub struct AdminCredentials {
    pub url: String,
    pub consumer: String,
    pub token_file: String,
}

/// Bootstrap coordinates used for store administration. They stay outside the
/// selected store to avoid circular authentication.
pub fn admin_credentials() -> Result<AdminCredentials, SkarbiecError> {
    let url = crate::config_file::resolve(
        "STADO_CREDENTIALS_ADMIN_URL",
        "credentials.admin.url",
        crate::config::skarbiec_url(),
    );
    let consumer = crate::config_file::resolve(
        "STADO_CREDENTIALS_ADMIN_CONSUMER",
        "credentials.admin.consumer",
        "local-operator",
    );
    let token_file = crate::config_file::resolve(
        "STADO_CREDENTIALS_ADMIN_TOKEN_FILE",
        "credentials.admin.token_file",
        "~/.stado/local-operator-skarbiec-token",
    );
    let token_file = crate::config_file::expand_tilde(&token_file)
        .to_string_lossy()
        .to_string();
    if consumer.trim().is_empty() || token_file.trim().is_empty() {
        return Err(SkarbiecError::Deployment(
            "credentials.admin.consumer and credentials.admin.token_file must be non-empty"
                .to_string(),
        ));
    }
    Ok(AdminCredentials {
        url,
        consumer,
        token_file,
    })
}

fn scheme_of(raw: &str) -> String {
    raw.split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or(raw)
        .to_string()
}

fn unsupported(scheme: &str) -> SkarbiecError {
    SkarbiecError::Deployment(format!(
        "unsupported credential store {scheme:?}; set {ENV_STORE} to skarbiec or file://<absolute-path>"
    ))
}

/// Selector persisted in the config file. This is deliberately read on every
/// call: a successful migration updates the config in the same process.
pub fn configured_selector() -> Result<String, SkarbiecError> {
    let Some(path) = crate::config_file::find_config_file() else {
        return Ok(DEFAULT_STORE.to_string());
    };
    let body = std::fs::read_to_string(&path).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot read Stado config {}: {source}",
            path.display()
        ))
    })?;
    let document: Value = serde_json::from_str(&body).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "Stado config {} is not valid JSON: {source}",
            path.display()
        ))
    })?;
    let root = document.as_object().ok_or_else(|| {
        SkarbiecError::Deployment(format!(
            "Stado config {} must contain a JSON object",
            path.display()
        ))
    })?;
    let raw = root
        .get("credentials")
        .and_then(|section| section.get("store"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_STORE)
        .trim();
    parse_selector(raw)?;
    Ok(raw.to_string())
}

/// Requested selector: environment override first, persisted config second.
pub fn requested_selector() -> Result<String, SkarbiecError> {
    match std::env::var(ENV_STORE) {
        Ok(value) if !value.trim().is_empty() => {
            parse_selector(value.trim())?;
            Ok(value.trim().to_string())
        }
        _ => configured_selector(),
    }
}

pub(crate) fn parse_selector(raw: &str) -> Result<Backend, SkarbiecError> {
    let raw = raw.trim();
    if raw.is_empty() || raw == DEFAULT_STORE {
        return Ok(Backend::Skarbiec { url: None });
    }
    if let Some(rest) = raw.strip_prefix("skarbiec://") {
        let rest = rest.trim();
        let url = (!rest.is_empty()).then(|| rest.to_string());
        return Ok(Backend::Skarbiec { url });
    }
    if let Some(path) = raw.strip_prefix("file://") {
        return file_backend(path);
    }
    if raw.starts_with('/') {
        return file_backend(raw);
    }
    Err(unsupported(&scheme_of(raw)))
}

pub(crate) fn selected() -> Result<Backend, SkarbiecError> {
    let configured = parse_selector(&configured_selector()?)?;
    let requested = parse_selector(&requested_selector()?)?;
    if requested != configured {
        return Err(SkarbiecError::Deployment(format!(
            "credential store change pending ({} -> {}); run `stado secrets migrate` before credential access",
            configured.locator(),
            requested.locator()
        )));
    }
    Ok(requested)
}

fn file_backend(path: &str) -> Result<Backend, SkarbiecError> {
    let path = path.trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err(unsupported("file (an absolute path is required)"));
    }
    Ok(Backend::File {
        path: PathBuf::from(path),
    })
}

fn configured_client(url: Option<&str>) -> Result<Client, SkarbiecError> {
    Client::direct(
        url.unwrap_or_else(|| crate::config::skarbiec_url()),
        crate::config::skarbiec_consumer(),
        crate::config::skarbiec_token_file(),
    )
}

/// Read one item with the configured consumer grant through the selected
/// store. This is what `Client::configured_item` callers route through.
pub async fn read_item(id: &str) -> Result<Value, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url } => configured_client(url.as_deref())?.read_item(id).await,
        Backend::File { path } => file::file_read_item(&path, id),
    }
}

/// Resolve one optional string field through the selected store. A missing
/// item or field is `None`; transport, schema, and authorization failures
/// remain explicit errors.
pub async fn read_string(id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url } => {
            configured_client(url.as_deref())?
                .read_string(id, field)
                .await
        }
        Backend::File { path } => file::file_read_string(&path, id, field),
    }
}

/// Read one item through the selected store for callers that already carry
/// their own Skarbiec coordinates. Under the skarbiec backend the supplied
/// triple is used exactly as before; under the file backend the item comes
/// from the store file instead.
pub async fn read_item_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
) -> Result<Value, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(store_url.as_deref().unwrap_or(url), consumer, token_file)?
                .read_item(id)
                .await
        }
        Backend::File { path } => file::file_read_item(&path, id),
    }
}

/// The broker's base URL when the selected store is a Skarbiec, and `None`
/// when it is a file. Exposed for diagnostics that need to talk to the broker
/// directly rather than through a `Client`, which would need a grant the probe
/// deliberately does without.
pub fn skarbiec_url() -> Option<String> {
    match selected().ok()? {
        Backend::Skarbiec { url } => {
            Some(url.unwrap_or_else(|| crate::config::skarbiec_url().to_string()))
        }
        Backend::File { .. } => None,
    }
}
