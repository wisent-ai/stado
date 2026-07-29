//! User-facing configuration file for stado.
//!
//! Port of `stado/config_file.py`. Resolution order for every setting:
//! environment variable wins, then the config file, then the built-in
//! default. The file is plain JSON (no new dependency) and is searched at,
//! in order: $STADO_CONFIG, ./stado.config.json, ~/.config/stado/config.json,
//! ~/.stado/config.json.
//!
//! Structured sections (storage/providers/azure/dashboard/alerts/billing)
//! are flattened onto the flat constant names config.rs already consumes, so
//! no consumer changes are required to adopt a file-driven deployment.
//!
//! The file is loaded once and cached process-wide (Python `_CACHE`), via
//! [`std::sync::OnceLock`]. A parse failure is NOT cached — the next call
//! retries, mirroring the Python behavior where `_CACHE["loaded"]` stays
//! None after a `ValueError`.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value};

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

struct Cache {
    path: Option<PathBuf>,
    data: Map<String, Value>,
}

static CACHE: OnceLock<Cache> = OnceLock::new();

/// Expand a leading `~` / `~/` using $HOME (Python `os.path.expanduser`).
pub(crate) fn expand_tilde(entry: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if entry == "~" {
        if let Some(home) = home() {
            return home;
        }
    } else if let Some(rest) = entry.strip_prefix("~/") {
        if let Some(home) = home() {
            return home.join(rest);
        }
    }
    PathBuf::from(entry)
}

/// Locate the config file: $STADO_CONFIG override first (must exist), then
/// the candidate list. Returns None when no file exists.
pub fn find_config_file() -> Option<PathBuf> {
    let override_ = std::env::var(FILE_ENV).unwrap_or_default();
    let override_ = override_.trim();
    if !override_.is_empty() {
        let candidate = expand_tilde(override_);
        return candidate.exists().then_some(candidate);
    }
    for entry in CANDIDATES {
        let candidate = expand_tilde(entry);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Parse a config file without touching the process-wide cache.
fn load_uncached(path: &std::path::Path) -> Result<Map<String, Value>, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|exc| ConfigError::Invalid {
        path: path.to_path_buf(),
        message: exc.to_string(),
    })?;
    let data: Value = serde_json::from_str(&text).map_err(|exc| ConfigError::Invalid {
        path: path.to_path_buf(),
        message: exc.to_string(),
    })?;
    match data {
        Value::Object(map) => Ok(map),
        _ => Err(ConfigError::NotAnObject(path.to_path_buf())),
    }
}

/// Load and cache the config file. Returns an empty map when no file
/// exists. Parse errors are returned (not cached) so a later call retries.
pub fn load_config_file() -> Result<&'static Map<String, Value>, ConfigError> {
    if let Some(cache) = CACHE.get() {
        return Ok(&cache.data);
    }
    let path = find_config_file();
    let data = match &path {
        None => Map::new(),
        Some(path) => load_uncached(path)?,
    };
    // First writer wins under a race; both computed equivalent results.
    let _ = CACHE.set(Cache { path, data });
    Ok(&CACHE.get().expect("cache just initialized").data)
}

/// The path of the loaded config file, or None when running file-less.
/// Mirrors Python `config_path()`: forces a load first.
pub fn config_path() -> Result<Option<PathBuf>, ConfigError> {
    load_config_file()?;
    Ok(CACHE.get().and_then(|cache| cache.path.clone()))
}

/// Dotted-key walk over a JSON object; None when any segment is missing or
/// an intermediate value is not an object (Python `_get`).
fn get_in<'a>(data: &'a Map<String, Value>, dotted: &str) -> Option<&'a Value> {
    let mut current: Option<&Value> = None;
    for (index, part) in dotted.split('.').enumerate() {
        let map = if index == 0 {
            data
        } else {
            current?.as_object()?
        };
        current = map.get(part);
    }
    current
}

/// Read a dotted key (e.g. `storage.gcs.bucket`) from the loaded file.
///
/// Python signature is `get(dotted, fallback=None)`; here the Option is the
/// fallback. Panics on a malformed config file (Python raises ValueError at
/// the equivalent call site).
pub fn get(dotted: &str) -> Option<Value> {
    let data = load_config_file().expect("invalid stado config file");
    get_in(data, dotted).cloned()
}

/// Python truthiness for JSON values (used by `validate`).
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// env > config file (dotted) > built-in default.
///
/// A set-but-empty environment variable counts as unset (Python
/// `value != ""`). Config file values are stringified: strings as-is,
/// numbers/bools via their JSON rendering; anything else (array, object,
/// null) falls through to the default. Panics on a malformed config file.
pub fn resolve(env_name: &str, dotted: &str, default: &str) -> String {
    if let Ok(value) = std::env::var(env_name) {
        if !value.is_empty() {
            return value;
        }
    }
    let data = load_config_file().expect("invalid stado config file");
    match get_in(data, dotted) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => default.to_string(),
    }
}

/// List-valued resolve: env comma-list > config file list > default.
///
/// Env values split on commas with each part trimmed and empties dropped.
/// Config file values must be a JSON array; items are stringified
/// (Python `str(part).strip()`), trimmed, and empties dropped. Panics on a
/// malformed config file.
pub fn resolve_list(env_name: &str, dotted: &str, default: &[&str]) -> Vec<String> {
    if let Ok(value) = std::env::var(env_name) {
        if !value.is_empty() {
            return value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    let data = load_config_file().expect("invalid stado config file");
    if let Some(Value::Array(items)) = get_in(data, dotted) {
        return items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
    }
    default.iter().map(|s| s.to_string()).collect()
}

fn unresolved_placeholders(value: &Value, path: &str, problems: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key.starts_with('_') {
                    continue;
                }
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                unresolved_placeholders(value, &child, problems);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                unresolved_placeholders(value, &format!("{path}[{index}]"), problems);
            }
        }
        Value::String(value) if value.contains('<') && value.contains('>') => {
            problems.push(format!(
                "{path} contains unresolved placeholder {value:?}; replace it before deployment"
            ));
        }
        _ => {}
    }
}

fn catalog_variant(
    kind: crate::capabilities::RuntimeFacet,
    value: Option<&Value>,
    label: &str,
    problems: &mut Vec<String>,
) -> Option<&'static crate::capabilities::CapabilityVariant> {
    let value = value.filter(|value| !value.is_null())?;
    let variant = value
        .as_str()
        .and_then(|name| crate::capabilities::configurable_variant(kind, name));
    if variant.is_none() {
        let choices = crate::capabilities::configurable_ids(kind)
            .collect::<Vec<_>>()
            .join("|");
        problems.push(format!("{label} must be {choices}, got {value:?}"));
    }
    variant
}

fn validate_variant_config(
    root: &Map<String, Value>,
    variant: &crate::capabilities::CapabilityVariant,
    backup: bool,
    problems: &mut Vec<String>,
) {
    for field in variant.config {
        let required = if backup {
            field.backup_required
        } else {
            field.required
        };
        let path = if backup {
            field.backup_path
        } else {
            Some(field.path)
        };
        if !required {
            continue;
        }
        let configured = path
            .and_then(|path| get_in(root, path))
            .is_some_and(py_truthy);
        let fallback = (!backup)
            .then_some(field.fallback_path)
            .flatten()
            .and_then(|path| get_in(root, path))
            .is_some_and(py_truthy);
        if !configured && !fallback {
            problems.push(format!(
                "{}.backend={} needs {}",
                if backup { "storage.backup" } else { "storage" },
                variant.id,
                path.unwrap_or(field.path)
            ));
        }
    }
}

/// Structural validation of a config dict; returns a list of problems.
pub fn validate(data: &Value) -> Vec<String> {
    let mut problems = crate::capabilities::validate_catalog();
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    unresolved_placeholders(data, "", &mut problems);
    let primary_field = crate::capabilities::STORAGE_BACKEND_CONFIG;
    let primary = catalog_variant(
        crate::capabilities::RuntimeFacet::Storage,
        get_in(root, primary_field.path),
        primary_field.path,
        &mut problems,
    );
    if let Some(variant) = primary {
        validate_variant_config(root, variant, false, &mut problems);
    }

    let backup_path = primary_field
        .backup_path
        .expect("storage backend catalog entry must define its backup path");
    let backup_variant = catalog_variant(
        crate::capabilities::RuntimeFacet::Storage,
        get_in(root, backup_path),
        backup_path,
        &mut problems,
    );
    if let Some(variant) = backup_variant {
        validate_variant_config(root, variant, true, &mut problems);
    }

    let primary_adapter = primary.and_then(|variant| match variant.adapter {
        crate::capabilities::RuntimeAdapter::Storage(adapter) => Some(adapter),
        _ => None,
    });
    let backup_adapter = backup_variant.and_then(|variant| match variant.adapter {
        crate::capabilities::RuntimeAdapter::Storage(adapter) => Some(adapter),
        _ => None,
    });
    if let Some(required) = primary_adapter.and_then(|adapter| adapter.required_backup()) {
        if backup_adapter != Some(required) {
            problems.push(format!(
                "{} cutover requires storage.backup.backend={}; the replica is read fallback only and is never promoted automatically",
                primary.map(|variant| variant.id).unwrap_or("selected storage"),
                required.id()
            ));
        }
    }
    let configured_providers = match get_in(root, crate::capabilities::PROVIDERS_CONFIG.path)
        .filter(|value| !value.is_null())
    {
        Some(Value::Array(providers)) if providers.is_empty() => {
            problems
                .push("providers must be a non-empty list of enabled provider names".to_string());
            &[]
        }
        Some(Value::Array(providers)) => providers.as_slice(),
        Some(_) => {
            problems.push("providers must be an array of enabled provider names".to_string());
            &[]
        }
        None => &[],
    };
    let disabled_providers = match get_in(root, crate::capabilities::DISABLED_PROVIDERS_CONFIG.path)
        .filter(|value| !value.is_null())
    {
        Some(Value::Array(providers)) => providers.as_slice(),
        Some(_) => {
            problems
                .push("providers_disabled must be an array of fenced provider names".to_string());
            &[]
        }
        None => &[],
    };
    let mut enabled = std::collections::BTreeSet::new();
    let mut enabled_names = std::collections::BTreeSet::new();
    for provider in configured_providers {
        let Some(name) = provider.as_str() else {
            problems.push("providers entries must be provider names".to_string());
            continue;
        };
        if !enabled_names.insert(name) {
            problems.push(format!("providers contains duplicate provider {name:?}"));
            continue;
        }
        let Some(canonical) = crate::capabilities::configurable_variant(
            crate::capabilities::RuntimeFacet::Compute,
            name,
        )
        .and_then(|variant| variant.provider) else {
            problems.push(format!("unknown provider: {provider:?}"));
            continue;
        };
        if !enabled.insert(canonical) {
            problems.push(format!(
                "providers entries {name:?} and an earlier alias identify the same provider"
            ));
        }
    }
    let mut disabled = std::collections::BTreeSet::new();
    let mut disabled_names = std::collections::BTreeSet::new();
    for provider in disabled_providers {
        let Some(name) = provider.as_str() else {
            problems.push("providers_disabled entries must be provider names".to_string());
            continue;
        };
        if !disabled_names.insert(name) {
            problems.push(format!(
                "providers_disabled contains duplicate provider {name:?}"
            ));
            continue;
        }
        let Some(canonical) = crate::capabilities::configurable_variant(
            crate::capabilities::RuntimeFacet::Compute,
            name,
        )
        .and_then(|variant| variant.provider) else {
            problems.push(format!("unknown disabled provider: {provider:?}"));
            continue;
        };
        if !disabled.insert(canonical) {
            problems.push(format!(
                "providers_disabled entries {name:?} and an earlier alias identify the same provider"
            ));
        }
    }
    for provider in enabled.intersection(&disabled) {
        problems.push(format!(
            "provider {provider:?} cannot be both enabled in providers and fenced in providers_disabled"
        ));
    }
    let active_providers = enabled.iter().copied().collect::<Vec<_>>();
    for provider in &active_providers {
        let Some(variant) = crate::capabilities::variant(
            crate::capabilities::RuntimeFacet::Compute,
            provider.as_str(),
        ) else {
            continue;
        };
        for field in variant.config.iter().filter(|field| field.required) {
            let configured = get_in(root, field.path).is_some_and(py_truthy)
                || field
                    .fallback_path
                    .and_then(|path| get_in(root, path))
                    .is_some_and(py_truthy);
            if !configured {
                problems.push(format!(
                    "{} provider needs {} (environment override {})",
                    provider, field.path, field.env
                ));
            }
        }
    }
    let cloud_agent_provider = [
        crate::capabilities::ProviderId::Gcp,
        crate::capabilities::ProviderId::Aws,
        crate::capabilities::ProviderId::Azure,
    ]
    .iter()
    .any(|provider| active_providers.contains(provider));
    if cloud_agent_provider {
        let release_api = get_in(root, "release.api_url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if release_api.is_empty() {
            problems.push(
                "cloud agents need explicit release.api_url for the public Stado release endpoint"
                    .to_string(),
            );
        } else if !release_api.starts_with("https://") {
            problems.push("release.api_url must use HTTPS".to_string());
        }
        for key in ["release.version", "release.platform"] {
            let value = get_in(root, key)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if value.is_empty()
                || value.trim() != value
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            {
                problems.push(format!(
                    "{key} must be an exact non-empty release coordinate containing only letters, digits, '.', '_' or '-'"
                ));
            }
        }
    }
    let azure_provider = active_providers.contains(&crate::capabilities::ProviderId::Azure);
    if azure_provider {
        if !get_in(root, "deployment.id").is_some_and(py_truthy) {
            problems.push(
                "Azure control plane needs deployment.id for dashboard RLS and trusted-proxy \
                 deployment binding"
                    .to_string(),
            );
        }
        for (key, remedy) in [
            (
                "secrets.skarbiec.url",
                "Azure control plane needs secrets.skarbiec.url to resolve service credentials",
            ),
            (
                "secrets.skarbiec.consumer",
                "Azure control plane needs a dedicated secrets.skarbiec.consumer",
            ),
            (
                "secrets.skarbiec.token_file",
                "Azure control plane needs an owner-only secrets.skarbiec.token_file",
            ),
        ] {
            if !get_in(root, key).is_some_and(py_truthy) {
                problems.push(remedy.to_string());
            }
        }
    }
    if azure_provider
        && get_in(root, "secrets.skarbiec.consumer").and_then(Value::as_str)
            != Some("stado-control-plane")
    {
        problems.push(
            "Azure coordinator/dashboard must use the dedicated read-only \
             secrets.skarbiec.consumer stado-control-plane"
                .to_string(),
        );
    }
    if azure_provider {
        let agent_url = get_in(root, "agent.skarbiec.url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let agent_consumer = get_in(root, "agent.skarbiec.consumer")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !agent_url.starts_with("https://") {
            problems.push(
                "agent.skarbiec.url must be an HTTPS Skarbiec endpoint reachable from Azure VMs"
                    .to_string(),
            );
        }
        if agent_consumer.is_empty()
            || matches!(
                agent_consumer,
                "stado-control-plane" | "stado-local-agent" | "stado-azure-agent"
            )
        {
            problems.push(
                "Azure dispatch requires a newly scoped workload-agent consumer distinct from \
                 control-plane, local-agent, and revoked legacy Azure-agent grants"
                    .to_string(),
            );
        }
        if !get_in(root, "agent.skarbiec.token_file").is_some_and(py_truthy) {
            problems.push(
                "agent.skarbiec.token_file is required; Stado cannot dispatch Azure VMs \
                 without an operator-provided owner-only workload grant"
                    .to_string(),
            );
        }
        let agent_items = get_in(root, "agent.skarbiec.items")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty());
        if !agent_items.is_some_and(|items| {
            items.iter().all(|item| {
                item.as_str().is_some_and(|name| {
                    !name.is_empty() && !matches!(name, "stado-aws" | "stado-azure" | "stado-gcp")
                })
            })
        }) {
            problems.push(
                "agent.skarbiec.items must be a non-empty workload-only string array and must \
                 not contain cloud-provider credential items"
                    .to_string(),
            );
        }
    }
    let configured_items = get_in(root, "agent.skarbiec.items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match get_in(root, "agent.skarbiec.secret_fields") {
        None => {}
        Some(Value::Array(fields)) => {
            for entry in fields {
                let Some(reference) = entry.as_str() else {
                    problems.push(
                        "agent.skarbiec.secret_fields entries must be item#field strings"
                            .to_string(),
                    );
                    continue;
                };
                let Some((item, field)) = reference.split_once('#') else {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must be item#field"
                    ));
                    continue;
                };
                if item.is_empty()
                    || field.is_empty()
                    || reference.matches('#').count() != std::iter::once(()).count()
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must contain one non-empty item#field"
                    ));
                }
                if !configured_items
                    .iter()
                    .any(|configured| configured.as_str() == Some(item))
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} names an item absent from agent.skarbiec.items"
                    ));
                }
                if matches!(
                    item,
                    "stado-aws"
                        | "stado-azure"
                        | "stado-gcp"
                        | "stado-machine-api"
                        | "stado-service-api"
                        | "stado-host-health-api"
                ) || item.ends_with("-object-api")
                    || item.ends_with("-release-publisher")
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields must not expose infrastructure item {item:?} to jobs"
                    ));
                }
            }
        }
        Some(_) => problems.push(
            "agent.skarbiec.secret_fields must be an array of item#field strings".to_string(),
        ),
    }
    let messaging = get_in(root, "backend.messaging.skarbiec").and_then(Value::as_object);
    let messaging_consumer = messaging
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if messaging_consumer != "wisent-backend-business-messaging" {
        problems.push(
            "backend.messaging.skarbiec.consumer must be the dedicated wisent-backend-business-messaging consumer"
                .to_string(),
        );
    }
    let messaging_token_file = messaging
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if messaging_token_file.is_empty() {
        problems.push(
            "backend.messaging.skarbiec.token_file must name the owner-only messaging grant file"
                .to_string(),
        );
    }
    for other_path in [
        "secrets.skarbiec.token_file",
        "agent.skarbiec.token_file",
        "object_api.skarbiec.token_file",
        "release_api.skarbiec.token_file",
        "service_api.skarbiec.token_file",
    ] {
        if !messaging_token_file.is_empty()
            && get_in(root, other_path).and_then(Value::as_str) == Some(messaging_token_file)
        {
            problems.push(format!(
                "backend.messaging.skarbiec.token_file must be distinct from {other_path}"
            ));
        }
    }
    let required_messaging_items = [
        "wisent-backend-apns",
        "wisent-backend-fcm",
        "stado-supabase",
    ];
    let optional_email_item = "wisent-backend-email-provider";
    let messaging_items = messaging
        .and_then(|section| section.get("items"))
        .and_then(Value::as_array);
    if !messaging_items.is_some_and(|items| {
        required_messaging_items
            .iter()
            .all(|expected| items.iter().any(|item| item.as_str() == Some(expected)))
            && items.iter().all(|item| {
                item.as_str().is_some_and(|item| {
                    required_messaging_items.contains(&item) || item == optional_email_item
                })
            })
            && items.iter().enumerate().all(|(index, item)| {
                items
                    .iter()
                    .skip(index.saturating_add(usize::from(true)))
                    .all(|later| later != item)
            })
    }) {
        problems.push(
            "backend.messaging.skarbiec.items must contain exactly wisent-backend-apns, wisent-backend-fcm, and stado-supabase; wisent-backend-email-provider is optional"
                .to_string(),
        );
    }
    if messaging.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "backend.messaging.skarbiec.token is forbidden; store the grant only in its owner-only token_file"
                .to_string(),
        );
    }
    if let Err(push_problems) =
        crate::config::parse_backend_push_clients(get_in(root, "backend.push_clients"))
    {
        problems.extend(push_problems);
    }
    let push_skarbiec = get_in(root, "backend.push_skarbiec").and_then(Value::as_object);
    if push_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "backend.push_skarbiec.url, when set, must be a non-empty verifier endpoint"
                .to_string(),
        );
    }
    if push_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "backend.push_skarbiec.consumer must be the dedicated verifier {:?}",
            crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER
        ));
    }
    let push_token_file = push_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if push_token_file.is_empty() {
        problems.push(
            "backend.push_skarbiec.token_file must name the owner-only push verifier grant file"
                .to_string(),
        );
    }
    for other_path in [
        "secrets.skarbiec.token_file",
        "agent.skarbiec.token_file",
        "backend.messaging.skarbiec.token_file",
        "object_api.skarbiec.token_file",
        "release_api.skarbiec.token_file",
        "machine_api.skarbiec.token_file",
        "service_api.skarbiec.token_file",
        "rate_limit.skarbiec.token_file",
    ] {
        if !push_token_file.is_empty()
            && get_in(root, other_path).and_then(Value::as_str) == Some(push_token_file)
        {
            problems.push(format!(
                "backend.push_skarbiec.token_file must be distinct from {other_path}"
            ));
        }
    }
    if push_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "backend.push_skarbiec.token is forbidden; store the grant only in its owner-only token_file"
                .to_string(),
        );
    }
    for item in ["wisent-app-push-router", "wisent-backend-push-router"] {
        if configured_items
            .iter()
            .any(|configured| configured.as_str() == Some(item))
        {
            problems.push(format!(
                "agent.skarbiec.items must not expose push verifier item {item:?} to jobs"
            ));
        }
    }
    let rate_limit = root.get("rate_limit").and_then(Value::as_object);
    if let Err(problem) = crate::rate_limit::parse_clients(
        rate_limit
            .and_then(|section| section.get("clients"))
            .cloned(),
    ) {
        problems.push(problem);
    }
    let rate_skarbiec = rate_limit
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if rate_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "rate_limit.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if rate_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "rate_limit.skarbiec.consumer must be the dedicated verifier {:?}",
            crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
        ));
    }
    let rate_token_file = rate_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if rate_token_file.is_empty() {
        problems.push(
            "rate_limit.skarbiec.token_file must name the owner-only rate-limit verifier grant file"
                .to_string(),
        );
    }
    for other_path in [
        "secrets.skarbiec.token_file",
        "agent.skarbiec.token_file",
        "backend.messaging.skarbiec.token_file",
        "backend.push_skarbiec.token_file",
        "object_api.skarbiec.token_file",
        "release_api.skarbiec.token_file",
        "machine_api.skarbiec.token_file",
        "service_api.skarbiec.token_file",
    ] {
        if !rate_token_file.is_empty()
            && get_in(root, other_path).and_then(Value::as_str) == Some(rate_token_file)
        {
            problems.push(format!(
                "rate_limit.skarbiec.token_file must be distinct from {other_path}"
            ));
        }
    }
    if rate_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "rate_limit.skarbiec.token is forbidden; store the grant only in its owner-only token_file"
                .to_string(),
        );
    }
    if configured_items
        .iter()
        .any(|configured| configured.as_str() == Some("trading-autonomy-rate-limit-api"))
    {
        problems.push(
            "agent.skarbiec.items must not expose rate-limit verifier items to jobs".to_string(),
        );
    }
    let integration = root.get("integration").and_then(Value::as_object);
    let integration_clients =
        crate::config::parse_integration_clients(get_in(root, "integration.clients"));
    match &integration_clients {
        Ok(clients) => {
            for item in clients.values().map(|client| client.item()) {
                if configured_items
                    .iter()
                    .any(|configured| configured.as_str() == Some(item))
                {
                    problems.push(format!(
                        "agent.skarbiec.items must not expose integration verifier item {item:?} to jobs"
                    ));
                }
            }
        }
        Err(integration_problems) => problems.extend(integration_problems.iter().cloned()),
    }
    if integration.is_some_and(|section| section.contains_key("providers")) {
        if let Err(provider_problems) =
            crate::config::parse_integration_providers(get_in(root, "integration.providers"))
        {
            problems.extend(provider_problems);
        }
    }
    let integration_skarbiec = integration
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if integration_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "integration.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if integration_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::INTEGRATION_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "integration.skarbiec.consumer must be the dedicated verifier {:?}",
            crate::config::INTEGRATION_API_VERIFIER_CONSUMER
        ));
    }
    let integration_token_file = integration_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if integration_token_file.is_empty() {
        problems.push(
            "integration.skarbiec.token_file must name the owner-only integration verifier grant file"
                .to_string(),
        );
    }
    for other_path in [
        "secrets.skarbiec.token_file",
        "agent.skarbiec.token_file",
        "backend.messaging.skarbiec.token_file",
        "backend.push_skarbiec.token_file",
        "rate_limit.skarbiec.token_file",
        "object_api.skarbiec.token_file",
        "release_api.skarbiec.token_file",
        "machine_api.skarbiec.token_file",
        "service_api.skarbiec.token_file",
    ] {
        if !integration_token_file.is_empty()
            && get_in(root, other_path).and_then(Value::as_str) == Some(integration_token_file)
        {
            problems.push(format!(
                "integration.skarbiec.token_file must be distinct from {other_path}"
            ));
        }
    }
    if integration_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "integration.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
    }
    let object_api = root.get("object_api").and_then(Value::as_object);
    if let Err(object_problems) = crate::config::parse_object_api_namespaces(
        object_api.and_then(|section| section.get("namespaces")),
    ) {
        problems.extend(object_problems);
    }
    let object_skarbiec = object_api
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if object_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "object_api.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if object_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::OBJECT_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "object_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
            crate::config::OBJECT_API_VERIFIER_CONSUMER
        ));
    }
    let object_token_file = object_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if object_token_file.is_empty() {
        problems.push(
            "object_api.skarbiec.token_file must name the owner-only verifier grant file"
                .to_string(),
        );
    }
    let control_token_file = get_in(root, "secrets.skarbiec.token_file")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !object_token_file.is_empty() && object_token_file == control_token_file {
        problems.push(
            "object_api.skarbiec.token_file must be distinct from the coordinator Skarbiec grant"
                .to_string(),
        );
    }
    if object_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "object_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
    }
    let release_api = root.get("release_api").and_then(Value::as_object);
    if let Err(release_problems) = crate::config::parse_release_publishers(
        release_api.and_then(|section| section.get("publishers")),
    ) {
        problems.extend(release_problems);
    }
    let release_skarbiec = release_api
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if release_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "release_api.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if release_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::RELEASE_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "release_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
            crate::config::RELEASE_API_VERIFIER_CONSUMER
        ));
    }
    let release_token_file = release_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if release_token_file.is_empty() {
        problems.push(
            "release_api.skarbiec.token_file must name the owner-only release verifier grant file"
                .to_string(),
        );
    }
    if !release_token_file.is_empty()
        && (release_token_file == control_token_file || release_token_file == object_token_file)
    {
        problems.push(
            "release_api.skarbiec.token_file must be distinct from coordinator and product-object verifier grants"
                .to_string(),
        );
    }
    if release_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "release_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
    }
    let machine_api = root.get("machine_api").and_then(Value::as_object);
    if let Err(machine_problems) = crate::config::parse_machine_api_clients(
        machine_api.and_then(|section| section.get("clients")),
    ) {
        problems.extend(machine_problems);
    }
    let machine_skarbiec = machine_api
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if machine_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "machine_api.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if machine_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::MACHINE_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "machine_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
            crate::config::MACHINE_API_VERIFIER_CONSUMER
        ));
    }
    let machine_token_file = machine_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if machine_token_file.is_empty() {
        problems.push(
            "machine_api.skarbiec.token_file must name the owner-only machine verifier grant file"
                .to_string(),
        );
    }
    if !machine_token_file.is_empty()
        && (machine_token_file == control_token_file
            || machine_token_file == object_token_file
            || machine_token_file == release_token_file
            || machine_token_file
                == get_in(root, "agent.skarbiec.token_file")
                    .and_then(Value::as_str)
                    .unwrap_or_default())
    {
        problems.push(
            "machine_api.skarbiec.token_file must be distinct from coordinator, workload-agent, object, and release verifier grants"
                .to_string(),
        );
    }
    if machine_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "machine_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
    }
    let service_api = root.get("service_api").and_then(Value::as_object);
    if let Err(service_problems) = crate::config::parse_service_deployers(
        service_api.and_then(|section| section.get("deployers")),
    ) {
        problems.extend(service_problems);
    }
    let service_skarbiec = service_api
        .and_then(|section| section.get("skarbiec"))
        .and_then(Value::as_object);
    if service_skarbiec
        .and_then(|section| section.get("url"))
        .is_some_and(|url| !py_truthy(url))
    {
        problems.push(
            "service_api.skarbiec.url, when set, must be a non-empty verifier endpoint".to_string(),
        );
    }
    if service_skarbiec
        .and_then(|section| section.get("consumer"))
        .and_then(Value::as_str)
        != Some(crate::config::SERVICE_API_VERIFIER_CONSUMER)
    {
        problems.push(format!(
            "service_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
            crate::config::SERVICE_API_VERIFIER_CONSUMER
        ));
    }
    let service_token_file = service_skarbiec
        .and_then(|section| section.get("token_file"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if service_token_file.is_empty() {
        problems.push(
            "service_api.skarbiec.token_file must name the owner-only service verifier grant file"
                .to_string(),
        );
    }
    if !service_token_file.is_empty()
        && (service_token_file == control_token_file
            || service_token_file == object_token_file
            || service_token_file == release_token_file
            || service_token_file == machine_token_file)
    {
        problems.push(
            "service_api.skarbiec.token_file must be distinct from coordinator, product-object, release, and machine verifier grants"
                .to_string(),
        );
    }
    if service_skarbiec.is_some_and(|section| section.contains_key("token")) {
        problems.push(
            "service_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
    }
    let local_provider = active_providers.contains(&crate::capabilities::ProviderId::Local);
    let has_workload_fields = get_in(root, "agent.skarbiec.secret_fields")
        .and_then(Value::as_array)
        .is_some_and(|fields| !fields.is_empty());
    if local_provider && has_workload_fields {
        if get_in(root, "agent.skarbiec.consumer").and_then(Value::as_str)
            != Some("stado-local-agent")
        {
            problems.push(
                "local workload secrets require agent.skarbiec.consumer stado-local-agent"
                    .to_string(),
            );
        }
        let agent_token = get_in(root, "agent.skarbiec.token_file")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let control_token = get_in(root, "secrets.skarbiec.token_file")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if agent_token.is_empty() || agent_token == control_token {
            problems.push(
                "local workload secrets require an agent.skarbiec.token_file distinct from the control-plane grant"
                    .to_string(),
            );
        }
    }
    let port = root
        .get("dashboard")
        .and_then(Value::as_object)
        .and_then(|d| d.get("port"));
    if let Some(port) = port.filter(|p| !p.is_null()) {
        let ok = port.as_i64().is_some_and(|p| p > 0 && p < 65536);
        if !ok {
            problems.push("dashboard.port must be an int between 1 and 65535".to_string());
        }
    }
    problems
}

/// A commented starting-point config, mirroring Python `template()`.
pub fn template() -> Value {
    let local = crate::capabilities::ProviderId::Local.as_str();
    let disabled =
        crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Compute)
            .filter(|provider| *provider != local)
            .collect::<Vec<_>>();
    serde_json::json!({
        "providers": [local],
        "providers_disabled": disabled,
        "storage": {
            "backend": "local",
            "local": {"path": "~/.stado/local-storage"},
            "backup": {
                "backend": "local",
                "local": {"path": "~/.stado/local-backup"}
            },
        },
        "deployment": {"id": "local-control-plane"},
        "backend": {
            "messaging": {
                "skarbiec": {
                    "consumer": "wisent-backend-business-messaging",
                    "token_file": "~/.stado/wisent-backend-business-messaging-skarbiec-token",
                    "items": [
                        "wisent-backend-apns",
                        "wisent-backend-fcm",
                        "stado-supabase"
                    ]
                }
            },
            "push_skarbiec": {
                "consumer": "stado-backend-push-api-verifier",
                "token_file": "~/.stado/stado-backend-push-api-verifier-skarbiec-token"
            },
            "push_clients": {
                "wisent-app": {
                    "item": "wisent-app-push-router",
                    "actions": ["register", "send", "status", "unregister"],
                    "paths": ["/api/backend/push/inactivity", "/api/backend/push/reachability", "/api/backend/push/register", "/api/backend/push/unregister"]
                },
                "wisent-backend": {
                    "item": "wisent-backend-push-router",
                    "actions": ["send"],
                    "paths": ["/api/backend/push/inactivity"]
                }
            }
        },
        "rate_limit": {
            "skarbiec": {
                "consumer": "stado-rate-limit-api-verifier",
                "token_file": "~/.stado/stado-rate-limit-api-verifier-skarbiec-token"
            },
            "clients": {
                "trading-autonomy": {
                    "consumer": "trading-autonomy-rate-limit-client",
                    "item": "trading-autonomy-rate-limit-api",
                    "namespaces": ["trading-autonomy"],
                    "actions": ["consume"]
                }
            }
        },
        "integration": {
            "skarbiec": {
                "consumer": "stado-integration-api-verifier",
                "token_file": "~/.stado/stado-integration-api-verifier-skarbiec-token"
            },
            "clients": {
                "content-platform-production": {"item": "content-platform-production-integration-api", "allowed_actions": ["content/umami.accounts", "content/umami.report", "content/umami.website.ensure", "content/resend.email.send", "content/resend.receiving.list", "content/resend.receiving.get", "content/resend.verification.code", "content/resend.deliverability.canary", "content/juicysms.number.order", "content/juicysms.order.status", "content/juicysms.order.cancel", "content/juicysms.balance", "content/stripe.account", "content/stripe.revenue.report", "content/stripe.transfer.create", "content/stripe.webhook.verify", "content/google.analytics.properties", "content/google.analytics.report", "content/google.firebase.apps", "content/media.external.import", "content/apify.tiktok.trends", "content/apify.instagram.search", "content/apify.twitter.search", "content/apify.youtube.search", "content/apify.pinterest.search", "content/apify.video.download", "content/apify.tiktok.metrics", "content/apify.tiktok.hashtag", "content/github.research.tex", "content/github.research.index", "content/reddit.top.images", "content/pinterest.search", "content/tokchart.sounds", "content/tokchart.hashtags", "content/gofile.resolve", "content/mega.resolve"]},
                "echo-production": {"item": "echo-production-integration-api", "allowed_actions": ["content/google.analytics.properties", "content/google.analytics.report", "content/google.firebase.apps", "content/resend.email.send", "content/resend.receiving.list", "content/resend.receiving.get", "content/resend.verification.code", "content/resend.deliverability.canary", "content/juicysms.number.order", "content/juicysms.order.status", "content/juicysms.order.cancel", "content/juicysms.balance", "content/apify.tiktok.trends", "content/apify.instagram.search", "content/apify.twitter.search", "content/apify.youtube.search", "content/apify.pinterest.search", "content/apify.video.download", "content/apify.tiktok.metrics", "content/apify.tiktok.hashtag", "content/serper.search", "content/github.research.tex", "content/github.research.index", "content/reddit.top.images", "content/pinterest.search", "content/tokchart.sounds", "content/tokchart.hashtags", "content/gofile.resolve", "content/mega.resolve", "echo-paid-ads/accounts.list", "echo-paid-ads/accounts.connect", "echo-paid-ads/campaigns.list", "echo-paid-ads/campaigns.get", "echo-paid-ads/campaigns.create", "echo-paid-ads/campaigns.mutate", "echo-paid-ads/entities.list", "echo-paid-ads/entities.create", "echo-paid-ads/entities.mutate", "echo-paid-ads/metrics.report", "echo-paid-ads/conversions.upload", "echo-paid-ads/webhook.verify", "echo-paid-ads/attribution.resolve"]},
                "wisent-backend-admin": {"item": "wisent-backend-admin-integration-api", "allowed_actions": ["backend/admin-jwt.verify"]},
                "wisent-backend": {"item": "wisent-backend-integration-api", "allowed_actions": ["backend/email.send", "backend/twilio.sms-send", "backend/twilio.sms-status", "backend/twilio.whatsapp-send", "backend/twilio.whatsapp-template", "backend/twilio.webhook-verify", "backend/content.pose-recipes", "backend/content.visual-profiles"]},
                "trading-tools": {"item": "trading-tools-integration-api", "allowed_actions": ["trading/send-whatsapp", "trading/verify-whatsapp-webhook"]},
                "most-service": {"item": "most-service-integration-api", "allowed_actions": ["most/send-sms"]},
                "oko": {"item": "oko-integration-api", "allowed_actions": ["oko/analytics.mobile.collect", "oko/experiments.assign"]},
                "people-rotator": {"item": "people-rotator-integration-api", "allowed_actions": ["people/prerequisites", "people/github.org.invite_member", "people/github.team.add_member", "people/github.repo.remove_collaborator", "people/github.org.remove_member", "people/github.membership.check", "people/github.org.revoke_fine_grained_pat_grants", "people/github.repo.transfer", "people/slack.user.invite", "people/slack.user.deactivate", "people/supabase.auth.invite_user", "people/supabase.auth.ban_user", "people/supabase.credentials.rotate", "people/weles.queue.enqueue"]},
                "singularity": {"item": "singularity-integration-api", "allowed_actions": ["singularity/resend_send_email", "singularity/sendgrid_send_email", "singularity/stripe_create_payment_link", "singularity/stripe_get_balance", "singularity/stripe_list_payments", "singularity/stripe_create_product", "singularity/stripe_refund_payment", "singularity/github_create_repo", "singularity/github_create_issue", "singularity/github_search_repos", "singularity/github_search_issues", "singularity/github_fork_repo", "singularity/github_star_repo", "singularity/github_get_user", "singularity/github_create_gist", "singularity/vercel_list_projects", "singularity/vercel_get_project", "singularity/vercel_create_project", "singularity/vercel_deploy", "singularity/vercel_list_deployments", "singularity/vercel_get_deployment", "singularity/vercel_list_domains", "singularity/vercel_add_domain", "singularity/vercel_remove_domain", "singularity/vercel_delete_project", "singularity/vercel_get_user", "singularity/twitter_post_tweet", "singularity/twitter_search_tweets", "singularity/twitter_get_mentions", "singularity/twitter_follow_user", "singularity/twitter_send_dm", "singularity/twitter_get_user_info", "singularity/twitter_like_tweet", "singularity/twitter_retweet", "singularity/namecheap_check_domain", "singularity/namecheap_register_domain", "singularity/namecheap_list_domains", "singularity/namecheap_get_dns", "singularity/namecheap_set_dns", "singularity/captcha_solve_recaptcha_v2", "singularity/captcha_solve_recaptcha_v3", "singularity/captcha_solve_hcaptcha", "singularity/captcha_solve_turnstile", "singularity/captcha_solve_image", "singularity/captcha_solve_funcaptcha", "singularity/huggingface_publish_dataset"]}
            }
        },
        "release": {"api_url": "", "version": "", "platform": ""},
        "secrets": {
            "skarbiec": {
                "consumer": "stado-control-plane",
                "token_file": "~/.stado/control-plane-skarbiec-token"
            }
        },
        "object_api": {
            "skarbiec": {
                "consumer": "stado-object-api-verifier",
                "token_file": "~/.stado/stado-object-api-verifier-skarbiec-token"
            },
            "namespaces": {
                "entitlements-rotator": {"item": "entitlements-rotator-object-api", "prefixes": ["skarbiec.vault.json"], "actions": ["get", "put"]},
                "echo": {"item": "echo-object-api", "prefix_policies": [
                    {"prefix": "aesthetics/", "actions": ["get", "stat"]},
                    {"prefix": "audio/", "actions": ["get", "put"]},
                    {"prefix": "batch-heroes/", "actions": ["get", "put"]},
                    {"prefix": "batch-videos/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "body-horror-requests/", "actions": ["get", "list", "stat"]},
                    {"prefix": "captions/", "actions": ["get", "put"]},
                    {"prefix": "civitai-references/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "daily-fill/", "actions": ["get", "list", "stat"]},
                    {"prefix": "eval-rejections/", "actions": ["get", "put"]},
                    {"prefix": "explainer/", "actions": ["get", "put"]},
                    {"prefix": "external-assets/", "actions": ["get", "put"]},
                    {"prefix": "gateway-output/", "actions": ["get", "list", "stat"]},
                    {"prefix": "lifestyle/", "actions": ["get", "put", "stat"]},
                    {"prefix": "lipsync/", "actions": ["get", "put", "stat"]},
                    {"prefix": "lora-compare/", "actions": ["get", "put", "stat"]},
                    {"prefix": "lora-training/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "loras/", "actions": ["get", "list", "put", "stat"]},
                    {"prefix": "needher/", "actions": ["get", "list", "put", "stat"]},
                    {"prefix": "needher-captions/", "actions": ["get", "put", "stat"]},
                    {"prefix": "needher-lifestyle/", "actions": ["get", "put", "stat"]},
                    {"prefix": "needher-watermarked/", "actions": ["get", "list", "put", "stat"]},
                    {"prefix": "passion-poses/", "actions": ["get", "put", "stat"]},
                    {"prefix": "pose-videos/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "scail/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "scene-videos/", "actions": ["get", "put", "stat"]},
                    {"prefix": "smoothmix/", "actions": ["delete", "get", "list", "put", "stat"]},
                    {"prefix": "training-images/", "actions": ["get", "list", "put", "stat"]},
                    {"prefix": "uploads/", "actions": ["get", "put"]}
                ]},
                "content-platform": {"item": "content-platform-object-api", "prefixes": ["ad-pipeline/", "aesthetics/", "batch-videos/", "caption-series/", "checkpoints/", "civitai-references/", "daily-fill/", "generated-days/", "generated-images/", "generated-videos/", "lifestyle/", "locations/", "lora-compare/", "passion/", "passion-poses/", "pipeline/", "pose-recipes/", "pose-videos/", "probierz/", "test/", "training/", "training-images/", "ugc/"], "actions": ["delete", "get", "list", "put", "stat"]},
                "growth-tactics": {"item": "growth-tactics-object-api", "prefixes": ["jobs/"], "actions": ["get", "list", "put"]},
                "needher": {"item": "needher-object-api", "prefixes": ["avatars/", "covers/", "daily-fill/", "daily-fill-alt-flash/", "daily-fill-noir-bw/", "feed-snapshot.json", "needher-captions/", "needher-lifestyle/", "needher-watermarked/", "pose-previews/", "pose-videos/"], "actions": ["get", "put", "stat"]},
                "oko": {"item": "oko-object-api", "prefixes": ["transcripts/"], "actions": ["get", "list", "put", "stat"]},
                "openenv": {"item": "openenv-object-api", "prefixes": ["datasets/", "models/", "pipeline/", "training/"], "actions": ["get", "put"]},
                "probierz": {"item": "probierz-object-api", "prefixes": ["capacity/", "inputs/", "results/"], "actions": ["get", "list", "put"]},
                "trading-autonomy": {"item": "trading-autonomy-object-api", "prefixes": ["agents/", "billing/", "media/"], "actions": ["delete", "get", "put"]},
                "trading-tools": {"item": "trading-tools-object-api", "prefixes": ["stock-context/"]},
                "weles": {"item": "weles-object-api", "prefixes": ["recordings/"]},
                "wisent-app": {"item": "wisent-app-object-api", "prefixes": ["character-previews/", "character-videos/", "characters/", "classifiers/", "dual-video/", "generation-inputs/", "generation-jobs/", "lora_tests/", "loras/", "manual-jobs/", "media/", "processing/", "rooms/", "training-inputs/", "verification/"], "actions": ["delete", "get", "list", "put"]},
                "wisent-backend": {"item": "wisent-backend-object-client", "prefixes": ["__healthcheck__", "activations/", "benchmarks/", "characters/", "contrastive_pairs/", "control_vectors/", "datasets/", "evaluation_results/", "images/", "optimization/", "personas/", "profiles/", "representations/", "state/", "traits/", "unique_personas/", "vector_stores/"], "actions": ["delete", "get", "list", "put", "stat"]},
                "wisent-images": {"item": "wisent-images-object-api", "prefixes": ["images/generated/", "models/base/sha256/", "models/loras/sha256/"], "actions": ["get", "put", "stat"]},
                "wisent-tools": {"item": "wisent-tools-object-api", "prefixes": ["activation-pairs/", "activations/", "datasets/", "evaluations/", "jobs/", "models/", "sweeps/"], "actions": ["delete", "get", "list", "put"]},
                "wisent-trade": {"item": "wisent-trade-object-api", "prefixes": ["agents/"]}
            }
        },
        "machine_api": {
            "skarbiec": {
                "consumer": "stado-machine-api-verifier",
                "token_file": "~/.stado/stado-machine-api-verifier-skarbiec-token"
            },
            "clients": {
                "echo": {
                    "item": "echo-machine-api",
                    "actions": ["cancel", "status", "submit"],
                    "targets": ["local"]
                }
            }
        },
        "release_api": {
            "skarbiec": {
                "consumer": "stado-release-api-verifier",
                "token_file": "~/.stado/stado-release-api-verifier-skarbiec-token"
            },
            "publishers": {
                "brama": {"item": "brama-release-publisher", "prefix": "brama/"},
                "compute-marketplace": {"item": "compute-marketplace-release-publisher", "prefix": "compute-marketplace/"},
                "image-video-router": {"item": "image-video-router-release-publisher", "prefix": "image-video-router/"},
                "jeden": {"item": "jeden-release-publisher", "prefix": "jeden/"},
                "oko": {"item": "oko-release-publisher", "prefix": "oko/"},
                "skarbiec": {"item": "skarbiec-release-publisher", "prefix": "skarbiec/"},
                "stado": {"item": "stado-release-publisher", "prefix": "stado/"},
                "trading-autonomy": {"item": "trading-autonomy-release-publisher", "prefix": "trading-autonomy/"},
                "wisent-backend": {"item": "wisent-backend-release-publisher", "prefix": "wisent-backend/"},
                "wisent-images": {"item": "wisent-images-release-publisher", "prefix": "wisent-images/"}
            }
        },
        "service_api": {
            "skarbiec": {
                "consumer": "stado-service-api-verifier",
                "token_file": "~/.stado/stado-service-api-verifier-skarbiec-token"
            },
            "deployers": {
                "weles": {
                    "consumer": "weles-service-deployer",
                    "item": "weles-service-deployer",
                    "services": ["com.wisent.weles-api"],
                    "actions": ["status", "restart"]
                },
                "compute-marketplace": {
                    "consumer": "compute-marketplace-service-deployer",
                    "item": "compute-marketplace-service-deployer",
                    "services": ["compute-marketplace-backend", "compute-marketplace-frontend"],
                    "actions": ["status", "restart"]
                },
                "wisent-backend": {
                    "consumer": "wisent-backend-release-deployer",
                    "item": "wisent-backend-release-deployer",
                    "services": ["wisent-backend"],
                    "actions": ["status", "restart"]
                }
            }
        },
        "agent": {
            "skarbiec": {
                "consumer": "stado-local-agent",
                "token_file": "~/.stado/local-agent-skarbiec-token",
                "items": [
                    "compute-marketplace-agent",
                    "trading-autonomy-agent-auth",
                    "trading-autonomy-media-router",
                    "trading-autonomy-model-router",
                    "wisent-backend-alert-router",
                    "wisent-backend-data-router",
                    "wisent-backend-inactivity-webhook",
                    "wisent-backend-media-router",
                    "wisent-backend-model-router",
                    "wisent-backend-object-client",
                    "wisent-backend-object-signing",
                    "wisent-backend-release-runner",
                    "wisent-backend-scheduler",
                    "wisent-trade-agent-email",
                    "wisent-trade-agent-model-router"
                ],
                "secret_fields": [
                    "compute-marketplace-agent#token",
                    "trading-autonomy-agent-auth#agent_auth_secret",
                    "trading-autonomy-media-router#token",
                    "trading-autonomy-model-router#token",
                    "wisent-backend-alert-router#token",
                    "wisent-backend-data-router#token",
                    "wisent-backend-inactivity-webhook#secret",
                    "wisent-backend-media-router#token",
                    "wisent-backend-model-router#token",
                    "wisent-backend-object-client#token",
                    "wisent-backend-object-signing#key",
                    "wisent-backend-release-runner#token",
                    "wisent-backend-scheduler#token",
                    "wisent-trade-agent-email#api-key",
                    "wisent-trade-agent-model-router#token",
                    "wisent-trade-agent-model-router#url"
                ]
            }
        },
        "dashboard": {"bind": "127.0.0.1", "port": 8765, "refresh_seconds": 10},
        "alerts": {"topic": ""},
        "billing": {"dataset": "billing_export", "table": "", "net_alert_usd": 100},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[test]
    fn find_config_file_honors_stado_config_env() {
        let file = write_temp_config("{}");
        std::env::set_var(FILE_ENV, file.path());
        assert_eq!(find_config_file(), Some(file.path().to_path_buf()));
        // A non-existent override yields None rather than falling through.
        std::env::set_var(FILE_ENV, "/nonexistent/stado-test-config.json");
        assert_eq!(find_config_file(), None);
        std::env::remove_var(FILE_ENV);
    }

    #[test]
    fn load_uncached_parses_objects_and_rejects_non_objects() {
        let file = write_temp_config(r#"{"a": {"b": "file-value"}}"#);
        let data = load_uncached(file.path()).unwrap();
        assert_eq!(
            get_in(&data, "a.b"),
            Some(&Value::String("file-value".into()))
        );
        assert_eq!(get_in(&data, "a.missing.deep"), None);
        assert_eq!(get_in(&data, "a.b.deeper"), None); // b is not an object

        let bad = write_temp_config(r#"["not", "an", "object"]"#);
        assert!(matches!(
            load_uncached(bad.path()),
            Err(ConfigError::NotAnObject(_))
        ));
        let broken = write_temp_config("{invalid json");
        assert!(matches!(
            load_uncached(broken.path()),
            Err(ConfigError::Invalid { .. })
        ));
    }

    #[test]
    fn resolve_prefers_env_then_file_then_default() {
        // Unique env var / dotted keys so no other test or real config file
        // can interfere regardless of test execution order.
        std::env::set_var("STADO_TEST_RESOLVE_ENV", "from-env");
        assert_eq!(
            resolve("STADO_TEST_RESOLVE_ENV", "zz.nope", "dflt"),
            "from-env"
        );
        // Empty-string env counts as unset.
        std::env::set_var("STADO_TEST_RESOLVE_ENV", "");
        assert_eq!(resolve("STADO_TEST_RESOLVE_ENV", "zz.nope", "dflt"), "dflt");
        std::env::remove_var("STADO_TEST_RESOLVE_ENV");
        assert_eq!(
            resolve("STADO_TEST_RESOLVE_MISSING", "zz.nope", "dflt"),
            "dflt"
        );
    }

    #[test]
    fn resolve_list_parses_comma_env() {
        std::env::set_var("STADO_TEST_LIST_ENV", " a, b ,,c,");
        assert_eq!(
            resolve_list("STADO_TEST_LIST_ENV", "zz.nope", &["x"]),
            ["a", "b", "c"]
        );
        std::env::remove_var("STADO_TEST_LIST_ENV");
        assert_eq!(
            resolve_list("STADO_TEST_LIST_MISSING", "zz.nope", &["x", "y"]),
            ["x", "y"]
        );
    }

    #[test]
    fn validate_catches_structural_problems() {
        assert!(validate(&template()).is_empty());
        assert!(
            validate(&serde_json::json!({"storage": {"backend": "ftp"}}))
                .iter()
                .any(|p| p.contains("gcs|azure|s3|local"))
        );
        assert!(
            validate(&serde_json::json!({"storage": {"backend": "gcs"}}))
                .iter()
                .any(|p| p.contains("storage.gcs.bucket"))
        );
        assert!(validate(&serde_json::json!({"storage": {"backend": "s3"}}))
            .iter()
            .any(|p| p.contains("storage.s3.bucket")));
        assert!(validate(&serde_json::json!({"providers": []}))
            .iter()
            .any(|p| p.contains("non-empty list")));
        assert!(
            validate(&serde_json::json!({"providers": ["gcp", "dcloud"]}))
                .iter()
                .any(|p| p.contains("unknown provider"))
        );
        assert!(validate(&serde_json::json!({"dashboard": {"port": 70000}}))
            .iter()
            .any(|p| p.contains("dashboard.port")));
        assert!(
            validate(&serde_json::json!({"dashboard": {"port": "8765"}}))
                .iter()
                .any(|p| p.contains("dashboard.port"))
        );
        // Top-level "bucket" satisfies the GCS backend requirement even when
        // unrelated mandatory product-boundary sections are absent.
        let problems = validate(&serde_json::json!({
            "storage": {"backend": "gcs"}, "bucket": "stado"
        }));
        assert!(
            !problems
                .iter()
                .any(|problem| problem.contains("storage.gcs.bucket")),
            "{problems:?}"
        );
    }
}
