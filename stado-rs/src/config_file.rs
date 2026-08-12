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

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value};

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
///
/// Private, and it must stay private. A caller holding a string-path reader can
/// read a key nobody catalogued, and — the expensive direction — an operator can
/// write a key nobody reads: that is precisely how `storage.stado.ca_file` came
/// to sit in the deployed configuration with no reader at all. Configuration
/// keys reach this walk only through a `ConfigField`: [`field_value`] for the
/// running process, [`field_in`] and [`binding_in`] for a document under
/// validation, and the dotted `get`/`resolve`/`resolve_list` primitives that
/// `config.rs` drives from catalog entries rather than from literals.
///
/// What legitimately stays on the string form is everything that is not a
/// configuration key: `schema_version` (the document's own contract), the
/// placeholder walk over arbitrary nodes, the section-presence gates that ask
/// only whether an operator declared a section at all, and the map sections
/// whose member names are operator data rather than settings —
/// `storage.<adapter>` and `integration.providers`.
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

/// Every binding a catalogued field declares, in the precedence the catalog
/// intends: the field's own environment override and path, then its fallback,
/// then its backup replica. `ConfigField` states the pairs; this states their
/// order once, so no reader can quietly grow a second one.
fn field_bindings(
    field: &crate::capabilities::ConfigField,
) -> [(Option<&'static str>, Option<&'static str>); 3] {
    [
        (Some(field.env), Some(field.path)),
        (field.fallback_env, field.fallback_path),
        (field.backup_env, field.backup_path),
    ]
}

/// Decode an environment override the way the catalog says the key reads: a
/// scalar verbatim, a list as a trimmed comma split, a document as the JSON its
/// parser expects. A set-but-empty variable counts as unset, matching
/// [`resolve`]. A malformed document override yields None rather than a panic;
/// the parser that owns the key already reports it against the key's own name.
fn env_value(name: &str, kind: crate::capabilities::ConfigValueKind) -> Option<Value> {
    let raw = std::env::var(name).ok().filter(|value| !value.is_empty())?;
    match kind {
        crate::capabilities::ConfigValueKind::Scalar => Some(Value::String(raw)),
        crate::capabilities::ConfigValueKind::List => Some(Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| Value::String(part.to_string()))
                .collect(),
        )),
        crate::capabilities::ConfigValueKind::Document => serde_json::from_str(&raw).ok(),
    }
}

/// The value of a catalogued configuration field, taken from the environment or
/// the loaded config file in the catalog's own precedence.
///
/// Reading by dotted string is no longer available, and the incident that took
/// it away is `storage.stado.ca_file`: it sat in the deployed configuration for
/// months, read by nothing, while `config validate` and `doctor` both passed the
/// whole time — because naming a key was free and binding it to a reader was
/// optional. Every validator compared the document against itself; none of them
/// could ask whether any code would ever consult the key, so the fleet published
/// its object API under a private authority and trusted nothing.
///
/// A field is a reader. Requiring one here is what lets the catalog answer "who
/// reads this?" for every setting, and what lets validation refuse a key that
/// nobody does.
///
/// A path that is present but null counts as unwritten, so a null primary falls
/// through to the fallback and backup bindings instead of shadowing them.
pub fn field_value(field: &crate::capabilities::ConfigField) -> Option<Value> {
    let data = load_config_file().expect("invalid stado config file");
    field_bindings(field).into_iter().find_map(|(env, path)| {
        env.and_then(|name| env_value(name, field.value_kind))
            .or_else(|| {
                path.and_then(|path| get_in(data, path))
                    .filter(|value| !value.is_null())
                    .cloned()
            })
    })
}

/// The value a *document* binds to a catalogued field's own path.
///
/// Validation judges the file an operator is about to deploy, so it deliberately
/// does not consult the environment: an override exported in the shell that runs
/// `stado config validate` is not a property of the document being validated.
fn field_in<'a>(
    root: &'a Map<String, Value>,
    field: &crate::capabilities::ConfigField,
) -> Option<&'a Value> {
    get_in(root, field.path)
}

/// The value a document binds to one of a field's alternate paths — the fallback
/// an older layout still honours, or the backup replica's mirror of the key.
/// Both paths come from the catalog; a None path means the field declares no
/// such binding, which is not the same as the binding being unset.
fn binding_in<'a>(root: &'a Map<String, Value>, path: Option<&str>) -> Option<&'a Value> {
    path.and_then(|path| get_in(root, path))
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
        let configured = binding_in(root, path).is_some_and(py_truthy);
        let fallback = binding_in(root, (!backup).then_some(field.fallback_path).flatten())
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

/// A key under a storage adapter must be one Stado actually reads.
///
/// `storage.stado.ca_file` sat in the deployed configuration for as long as the
/// fleet published its object API over TLS, and no code path ever read it. It
/// validated clean, `doctor` was satisfied, and the only storage URL that still
/// worked was a loopback one — so every host quietly addressed its own store
/// while both reported the same shared backend, and two machines held different
/// registries without either noticing. An unknown key is not a harmless extra:
/// it is a setting an operator believes is in effect.
///
/// The catalog is authoritative here in a way it is not for the rest of the
/// document. A storage adapter's `ConfigField` list names every key its backend
/// consumes, so anything else under that section is unread by construction, and
/// saying so at validation time is the difference between a typo and a month of
/// silent divergence. Sections whose adapter the catalog does not know are left
/// alone: this reports keys that cannot be read, never adapters it cannot judge.
fn unread_storage_keys(root: &Map<String, Value>, problems: &mut Vec<String>) {
    let Some(storage) = root.get("storage").and_then(Value::as_object) else {
        return;
    };
    for (adapter, section) in storage {
        let Some(section) = section.as_object() else {
            continue;
        };
        let Some(variant) =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Storage, adapter)
        else {
            continue;
        };
        let mut known = std::collections::BTreeSet::new();
        for field in variant.config {
            for path in [Some(field.path), field.fallback_path, field.backup_path]
                .into_iter()
                .flatten()
            {
                known.insert(path);
            }
        }
        for key in section.keys() {
            // Operators annotate these documents heavily, and a comment is not a
            // claim about behaviour.
            if key.starts_with('_') {
                continue;
            }
            let path = format!("storage.{adapter}.{key}");
            if !known.contains(path.as_str()) {
                problems.push(format!(
                    "{path} is not a key Stado reads; the {adapter:?} backend consumes only [{}]",
                    known.iter().copied().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
}

/// Structural validation of a config dict; returns a list of problems.
pub fn validate(data: &Value) -> Vec<String> {
    let mut problems = crate::capabilities::validate_catalog();
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    match root.get("schema_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(SCHEMA_VERSION) => {}
        Some(version) => problems.push(format!(
            "unsupported config schema_version {version}; expected {SCHEMA_VERSION}"
        )),
        None => problems.push(format!(
            "config schema_version is required; expected {SCHEMA_VERSION}"
        )),
    }
    unresolved_placeholders(data, "", &mut problems);
    unread_storage_keys(root, &mut problems);
    if let Some(store) = field_in(root, &crate::capabilities::CREDENTIALS_STORE_CONFIG) {
        match store.as_str().filter(|value| !value.trim().is_empty()) {
            Some(store) => {
                if let Err(error) = crate::credential_store::parse_selector(store) {
                    problems.push(error.to_string());
                }
            }
            None => problems.push("credentials.store must be a non-empty string".to_string()),
        }
    }
    for field in [
        &crate::capabilities::CREDENTIALS_ADMIN_CONSUMER_CONFIG,
        &crate::capabilities::CREDENTIALS_ADMIN_TOKEN_FILE_CONFIG,
    ] {
        if field_in(root, field)
            .is_some_and(|value| !value.as_str().is_some_and(|entry| !entry.trim().is_empty()))
        {
            problems.push(format!("{} must be a non-empty string", field.path));
        }
    }
    if let Some(channels) = field_in(root, &crate::capabilities::ALERT_CHANNELS_CONFIG) {
        match channels {
            Value::Array(values) => {
                let supported = crate::capabilities::configurable_ids(
                    crate::capabilities::RuntimeFacet::Alerts,
                )
                .collect::<std::collections::BTreeSet<_>>();
                for value in values {
                    match value.as_str() {
                        Some(channel) if supported.contains(channel) => {}
                        Some(channel) => problems.push(format!(
                            "alerts.channels contains unsupported channel {channel:?}"
                        )),
                        None => {
                            problems.push("alerts.channels entries must be strings".to_string())
                        }
                    }
                }
            }
            _ => problems.push("alerts.channels must be an array".to_string()),
        }
    }
    let primary_field = crate::capabilities::STORAGE_BACKEND_CONFIG;
    let primary = catalog_variant(
        crate::capabilities::RuntimeFacet::Storage,
        field_in(root, &primary_field),
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
        binding_in(root, primary_field.backup_path),
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
    let configured_providers = match field_in(root, &crate::capabilities::PROVIDERS_CONFIG)
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
    let disabled_providers = match field_in(root, &crate::capabilities::DISABLED_PROVIDERS_CONFIG)
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
            let configured = field_in(root, field).is_some_and(py_truthy)
                || binding_in(root, field.fallback_path).is_some_and(py_truthy);
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
        let api = field_in(root, &crate::capabilities::API_URL_CONFIG)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if api.is_empty() {
            problems.push(
                "cloud agents need explicit api.url for the canonical Stado endpoint".to_string(),
            );
        } else if !api.starts_with("https://") {
            problems.push("api.url must use HTTPS".to_string());
        }
        for field in [
            &crate::capabilities::RELEASE_VERSION_CONFIG,
            &crate::capabilities::RELEASE_PLATFORM_CONFIG,
        ] {
            let value = field_in(root, field)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if value.is_empty()
                || value.trim() != value
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            {
                problems.push(format!(
                    "{} must be an exact non-empty release coordinate containing only letters, digits, '.', '_' or '-'",
                    field.path
                ));
            }
        }
    }
    let azure_provider = active_providers.contains(&crate::capabilities::ProviderId::Azure);
    if azure_provider {
        if !field_in(root, &crate::capabilities::DEPLOYMENT_ID_CONFIG).is_some_and(py_truthy) {
            problems.push(
                "Azure control plane needs deployment.id for dashboard RLS and trusted-proxy \
                 deployment binding"
                    .to_string(),
            );
        }
        let control = crate::capabilities::SECRETS_SKARBIEC;
        for (field, remedy) in [
            (
                &control.url,
                "Azure control plane needs secrets.skarbiec.url to resolve service credentials",
            ),
            (
                &control.consumer,
                "Azure control plane needs a dedicated secrets.skarbiec.consumer",
            ),
            (
                &control.token_file,
                "Azure control plane needs an owner-only secrets.skarbiec.token_file",
            ),
        ] {
            if !field_in(root, field).is_some_and(py_truthy) {
                problems.push(remedy.to_string());
            }
        }
    }
    if azure_provider
        && field_in(root, &crate::capabilities::SECRETS_SKARBIEC.consumer).and_then(Value::as_str)
            != Some("stado-control-plane")
    {
        problems.push(
            "Azure coordinator/dashboard must use the dedicated read-only \
             secrets.skarbiec.consumer stado-control-plane"
                .to_string(),
        );
    }
    if azure_provider {
        let agent_url = field_in(root, &crate::capabilities::AGENT_SKARBIEC.url)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let agent_consumer = field_in(root, &crate::capabilities::AGENT_SKARBIEC.consumer)
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
        if !field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file).is_some_and(py_truthy) {
            problems.push(
                "agent.skarbiec.token_file is required; Stado cannot dispatch Azure VMs \
                 without an operator-provided owner-only workload grant"
                    .to_string(),
            );
        }
        let agent_items = field_in(root, &crate::capabilities::AGENT_SKARBIEC_ITEMS_CONFIG)
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
    let configured_items = field_in(root, &crate::capabilities::AGENT_SKARBIEC_ITEMS_CONFIG)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match field_in(
        root,
        &crate::capabilities::AGENT_SKARBIEC_SECRET_FIELDS_CONFIG,
    ) {
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
    let control_token_file = field_in(root, &crate::capabilities::SECRETS_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let object_token_file = field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let release_token_file = field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let machine_token_file = field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Section-presence gate rather than a key read: the checks below apply only
    // to a messaging section an operator chose to declare at all.
    let messaging = get_in(root, "backend.messaging.skarbiec").and_then(Value::as_object);
    if messaging.is_some() {
        let messaging_consumer = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC.consumer,
        )
        .and_then(Value::as_str)
        .unwrap_or_default();
        if messaging_consumer != "wisent-backend-business-messaging" {
            problems.push(
            "backend.messaging.skarbiec.consumer must be the dedicated wisent-backend-business-messaging consumer"
                .to_string(),
        );
        }
        let messaging_token_file = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC.token_file,
        )
        .and_then(Value::as_str)
        .unwrap_or_default();
        if messaging_token_file.is_empty() {
            problems.push(
            "backend.messaging.skarbiec.token_file must name the owner-only messaging grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !messaging_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(messaging_token_file)
            {
                problems.push(format!(
                    "backend.messaging.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
                ));
            }
        }
        let required_messaging_items = [
            "wisent-backend-apns",
            "wisent-backend-fcm",
            "stado-supabase",
        ];
        let optional_email_item = "wisent-backend-email-provider";
        let messaging_items = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC_ITEMS_CONFIG,
        )
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
    }
    let rate_limit = root.get("rate_limit").and_then(Value::as_object);
    if rate_limit.is_some() {
        if let Err(problem) = crate::rate_limit::parse_clients(
            field_in(root, &crate::capabilities::RATE_LIMIT_CLIENTS_CONFIG).cloned(),
        ) {
            problems.push(problem);
        }
        let rate_skarbiec = rate_limit
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "rate_limit.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "rate_limit.skarbiec.consumer must be the dedicated verifier {:?}",
                crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
            ));
        }
        let rate_token_file = field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.token_file)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if rate_token_file.is_empty() {
            problems.push(
            "rate_limit.skarbiec.token_file must name the owner-only rate-limit verifier grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::BACKEND_MESSAGING_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::MACHINE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !rate_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(rate_token_file)
            {
                problems.push(format!(
                    "rate_limit.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
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
                "agent.skarbiec.items must not expose rate-limit verifier items to jobs"
                    .to_string(),
            );
        }
    }
    let integration = root.get("integration").and_then(Value::as_object);
    if integration.is_some() {
        let integration_clients = crate::config::parse_integration_clients(field_in(
            root,
            &crate::capabilities::INTEGRATION_CLIENTS_CONFIG,
        ));
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
            if let Err(provider_problems) = crate::config::parse_integration_providers(field_in(
                root,
                &crate::capabilities::INTEGRATION_PROVIDERS_CONFIG,
            )) {
                problems.extend(provider_problems);
            }
        }
        let integration_skarbiec = integration
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "integration.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::INTEGRATION_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "integration.skarbiec.consumer must be the dedicated verifier {:?}",
                crate::config::INTEGRATION_API_VERIFIER_CONSUMER
            ));
        }
        let integration_token_file =
            field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
        if integration_token_file.is_empty() {
            problems.push(
            "integration.skarbiec.token_file must name the owner-only integration verifier grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::BACKEND_MESSAGING_SKARBIEC,
            crate::capabilities::RATE_LIMIT_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::MACHINE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !integration_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(integration_token_file)
            {
                problems.push(format!(
                    "integration.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
                ));
            }
        }
        if integration_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "integration.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
        }
    }
    let object_api = root.get("object_api").and_then(Value::as_object);
    if object_api.is_some() {
        if let Err(object_problems) = crate::config::parse_object_api_namespaces(field_in(
            root,
            &crate::capabilities::OBJECT_API_NAMESPACES_CONFIG,
        )) {
            problems.extend(object_problems);
        }
        let object_skarbiec = object_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "object_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::OBJECT_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "object_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::OBJECT_API_VERIFIER_CONSUMER
            ));
        }
        if object_token_file.is_empty() {
            problems.push(
                "object_api.skarbiec.token_file must name the owner-only verifier grant file"
                    .to_string(),
            );
        }
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
    }
    let release_api = root.get("release_api").and_then(Value::as_object);
    if release_api.is_some() {
        if let Err(release_problems) = crate::config::parse_release_publishers(field_in(
            root,
            &crate::capabilities::RELEASE_API_PUBLISHERS_CONFIG,
        )) {
            problems.extend(release_problems);
        }
        let release_skarbiec = release_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "release_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::RELEASE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "release_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::RELEASE_API_VERIFIER_CONSUMER
            ));
        }
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
    }
    let machine_api = root.get("machine_api").and_then(Value::as_object);
    if machine_api.is_some() {
        if let Err(machine_problems) = crate::config::parse_machine_api_clients(field_in(
            root,
            &crate::capabilities::MACHINE_API_CLIENTS_CONFIG,
        )) {
            problems.extend(machine_problems);
        }
        let machine_skarbiec = machine_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "machine_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::MACHINE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "machine_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::MACHINE_API_VERIFIER_CONSUMER
            ));
        }
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
                    == field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file)
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
    }
    let service_api = root.get("service_api").and_then(Value::as_object);
    if service_api.is_some() {
        if let Err(service_problems) = crate::config::parse_service_deployers(field_in(
            root,
            &crate::capabilities::SERVICE_API_DEPLOYERS_CONFIG,
        )) {
            problems.extend(service_problems);
        }
        let service_skarbiec = service_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "service_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::SERVICE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "service_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::SERVICE_API_VERIFIER_CONSUMER
            ));
        }
        let service_token_file =
            field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.token_file)
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
        let has_workload_fields = field_in(
            root,
            &crate::capabilities::AGENT_SKARBIEC_SECRET_FIELDS_CONFIG,
        )
        .and_then(Value::as_array)
        .is_some_and(|fields| !fields.is_empty());
        if local_provider && has_workload_fields {
            if field_in(root, &crate::capabilities::AGENT_SKARBIEC.consumer).and_then(Value::as_str)
                != Some("stado-local-agent")
            {
                problems.push(
                    "local workload secrets require agent.skarbiec.consumer stado-local-agent"
                        .to_string(),
                );
            }
            let agent_token = field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let control_token = field_in(root, &crate::capabilities::SECRETS_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if agent_token.is_empty() || agent_token == control_token {
                problems.push(
                "local workload secrets require an agent.skarbiec.token_file distinct from the control-plane grant"
                    .to_string(),
            );
            }
        }
    }
    let port = field_in(root, &crate::capabilities::DASHBOARD_PORT_CONFIG);
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
        "deployment": {"id": "local-control-plane"},
        "dashboard": {"bind": "localhost"}
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
        let template_problems = validate(&template());
        assert!(template_problems.is_empty(), "{template_problems:?}");
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
