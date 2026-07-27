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

/// Structural validation of a config dict; returns a list of problems.
pub fn validate(data: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    unresolved_placeholders(data, "", &mut problems);
    let storage = root.get("storage").and_then(Value::as_object);
    let backend = storage.and_then(|s| s.get("backend"));
    if let Some(backend) = backend.filter(|b| !b.is_null()) {
        let ok = backend.as_str().is_some_and(|name| {
            crate::capabilities::configurable_variant(
                crate::capabilities::CapabilityKind::Storage,
                name,
            )
            .is_some()
        });
        if !ok {
            let choices =
                crate::capabilities::configurable_ids(crate::capabilities::CapabilityKind::Storage)
                    .collect::<Vec<_>>()
                    .join("|");
            problems.push(format!(
                "storage.backend must be {choices}, got {backend:?}"
            ));
        }
    }
    if backend.and_then(Value::as_str) == Some("azure") {
        let account = storage
            .and_then(|s| s.get("azure"))
            .and_then(Value::as_object)
            .and_then(|azure| azure.get("account"));
        let container = storage
            .and_then(|s| s.get("azure"))
            .and_then(Value::as_object)
            .and_then(|azure| azure.get("container"));
        if !account.is_some_and(py_truthy) {
            problems.push(
                "storage.backend=azure needs storage.azure.account; provision the Azure storage \
                 account before cutover"
                    .to_string(),
            );
        }
        if !container.is_some_and(py_truthy) {
            problems.push("storage.backend=azure needs storage.azure.container".to_string());
        }
    }
    if backend.and_then(Value::as_str) == Some("gcs") {
        let gcs_bucket = storage
            .and_then(|s| s.get("gcs"))
            .and_then(Value::as_object)
            .and_then(|g| g.get("bucket"));
        let top_bucket = root.get("bucket");
        if !gcs_bucket.is_some_and(py_truthy) && !top_bucket.is_some_and(py_truthy) {
            problems.push("storage.backend=gcs needs storage.gcs.bucket".to_string());
        }
    }
    if backend.and_then(Value::as_str) == Some("s3") {
        let s3_bucket = storage
            .and_then(|s| s.get("s3"))
            .and_then(Value::as_object)
            .and_then(|g| g.get("bucket"));
        if !s3_bucket.is_some_and(py_truthy) {
            problems.push("storage.backend=s3 needs storage.s3.bucket".to_string());
        }
    }
    let backup = storage
        .and_then(|s| s.get("backup"))
        .and_then(Value::as_object);
    let backup_backend = backup
        .and_then(|value| value.get("backend"))
        .and_then(Value::as_str);
    if let Some(kind) = backup_backend {
        if crate::capabilities::configurable_variant(
            crate::capabilities::CapabilityKind::Storage,
            kind,
        )
        .is_none()
        {
            let choices =
                crate::capabilities::configurable_ids(crate::capabilities::CapabilityKind::Storage)
                    .collect::<Vec<_>>()
                    .join("|");
            problems.push(format!(
                "storage.backup.backend must be {choices}, got {kind:?}"
            ));
        }
    }
    if backend.and_then(Value::as_str) == Some("azure") && backup_backend != Some("s3") {
        problems.push(
            "Azure cutover requires storage.backup.backend=s3; the replica is read fallback only \
             and is never promoted automatically"
                .to_string(),
        );
    }
    if backup_backend == Some("s3") {
        let bucket = backup.and_then(|value| value.get("bucket"));
        let region = backup
            .and_then(|value| value.get("s3"))
            .and_then(Value::as_object)
            .and_then(|value| value.get("region"));
        if !bucket.is_some_and(py_truthy) {
            problems.push(
                "storage.backup.backend=s3 needs storage.backup.bucket; provision the bucket \
                 before deployment"
                    .to_string(),
            );
        }
        if !region.is_some_and(py_truthy) {
            problems.push("storage.backup.backend=s3 needs storage.backup.s3.region".to_string());
        }
    }
    if let Some(providers) = root.get("providers").filter(|p| !p.is_null()) {
        match providers.as_array() {
            Some(list) if !list.is_empty() => {
                for provider in list {
                    let ok = provider.as_str().is_some_and(|name| {
                        crate::capabilities::configurable_variant(
                            crate::capabilities::CapabilityKind::Compute,
                            name,
                        )
                        .is_some()
                    });
                    if !ok {
                        problems.push(format!("unknown provider: {provider:?}"));
                    }
                }
            }
            _ => problems.push("providers must be a non-empty list".to_string()),
        }
    }
    let configured_providers = root
        .get("providers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let disabled_providers = root
        .get("providers_disabled")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for disabled in disabled_providers {
        let Some(name) = disabled.as_str() else {
            problems.push("providers_disabled entries must be provider names".to_string());
            continue;
        };
        if !configured_providers
            .iter()
            .any(|provider| provider.as_str() == Some(name))
        {
            problems.push(format!(
                "providers_disabled contains {name:?}, but providers does not"
            ));
        }
    }
    if !configured_providers.is_empty()
        && configured_providers.iter().all(|provider| {
            disabled_providers
                .iter()
                .any(|disabled| disabled == provider)
        })
    {
        problems.push("providers_disabled fences every configured provider".to_string());
    }
    let azure_provider = configured_providers
        .iter()
        .any(|provider| provider.as_str() == Some("azure"))
        && !disabled_providers
            .iter()
            .any(|provider| provider.as_str() == Some("azure"));
    if azure_provider {
        for (key, remedy) in [
            (
                "azure.subscription_id",
                "azure provider needs azure.subscription_id for ARM",
            ),
            (
                "azure.vm_identity_id",
                "azure provider needs azure.vm_identity_id; provision a user-assigned identity \
                 with Blob Data Contributor and VM self-delete rights",
            ),
            (
                "azure.ssh_public_key",
                "azure provider needs azure.ssh_public_key for VM creation",
            ),
        ] {
            if !get_in(root, key).is_some_and(py_truthy) {
                problems.push(remedy.to_string());
            }
        }
        if !get_in(root, "deployment.id").is_some_and(py_truthy) {
            problems.push(
                "Azure control plane needs deployment.id for dashboard RLS and trusted-proxy \
                 deployment binding"
                    .to_string(),
            );
        }
        let release_base = get_in(root, "release.base_url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if release_base.is_empty() {
            problems.push(
                "Azure control plane needs explicit release.base_url; publish the Rust release \
                 tree before cutover"
                    .to_string(),
            );
        } else if !release_base.starts_with("https://") {
            problems.push("release.base_url must be HTTPS for Azure cutover".to_string());
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
        if agent_consumer != "stado-azure-agent" {
            problems.push(
                "agent.skarbiec.consumer must be stado-azure-agent with exact read scopes \
                 matching agent.skarbiec.items"
                    .to_string(),
            );
        }
        if !get_in(root, "agent.skarbiec.token_file").is_some_and(py_truthy) {
            problems.push(
                "agent.skarbiec.token_file is required; Stado cannot dispatch Azure VMs \
                 without the operator-provided owner-only grant"
                    .to_string(),
            );
        }
        let agent_items = get_in(root, "agent.skarbiec.items")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty());
        if !agent_items.is_some_and(|items| {
            items
                .iter()
                .all(|item| item.as_str().is_some_and(|name| !name.is_empty()))
                && (backup_backend != Some("s3")
                    || items.iter().any(|item| item.as_str() == Some("stado-aws")))
        }) {
            problems.push(
                "agent.skarbiec.items must be a non-empty string array and include stado-aws \
                 when S3 backup is enabled; mint stado-azure-agent with exactly these scopes"
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
    serde_json::json!({
        "providers": ["azure", "aws", "gcp"],
        "providers_disabled": ["aws", "gcp"],
        "storage": {
            "backend": "azure",
            "azure": {"account": "<storage-account>", "container": "stado"},
            "backup": {
                "backend": "s3",
                "bucket": "<backup-bucket>",
                "s3": {"region": "<backup-region>"}
            },
        },
        "azure": {
            "subscription_id": "",
            "resource_group": "wisent-compute",
            "locations": ["eastus", "westus3"],
            "vnet": "wisent-compute-vnet",
            "subnet": "wisent-compute-subnet",
            "nsg": "wisent-compute-nsg",
            "image_urn": "microsoft-dsvm:ubuntu-hpc:2204:latest",
            "vm_username": "wisent",
            "ssh_public_key": "",
            "vm_identity_id": "<managed-identity-resource-id>",
        },
        "deployment": {"id": "azure-control-plane"},
        "release": {
            "base_url": "https://<storage-account>.blob.core.windows.net/stado/releases/stado"
        },
        "secrets": {
            "skarbiec": {
                "url": "<control-plane-skarbiec-url>",
                "consumer": "stado-control-plane",
                "token_file": "~/.stado/control-plane-skarbiec-token"
            }
        },
        "agent": {
            "skarbiec": {
                "url": "https://<skarbiec-host>",
                "consumer": "stado-azure-agent",
                "token_file": "~/.stado/azure-agent-skarbiec-token",
                "items": [
                    "compute-marketplace-agent",
                    "stado-aws",
                    "stado-huggingface",
                    "stado-model-router",
                    "stado-wandb",
                    "trading-autonomy-web-runtime"
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
        // Top-level "bucket" satisfies the gcs backend requirement.
        assert!(validate(&serde_json::json!({
            "storage": {"backend": "gcs"}, "bucket": "stado"
        }))
        .is_empty());
    }
}
