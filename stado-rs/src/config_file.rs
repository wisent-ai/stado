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
pub const CANDIDATES: [&str; 3] =
    ["stado.config.json", "~/.config/stado/config.json", "~/.stado/config.json"];

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
        let map = if index == 0 { data } else { current?.as_object()? };
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

/// Structural validation of a config dict; returns a list of problems.
pub fn validate(data: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    let storage = root.get("storage").and_then(Value::as_object);
    let backend = storage.and_then(|s| s.get("backend"));
    if let Some(backend) = backend.filter(|b| !b.is_null()) {
        let ok = backend.as_str().is_some_and(|b| matches!(b, "gcs" | "azure" | "s3" | "local"));
        if !ok {
            problems
                .push(format!("storage.backend must be gcs|azure|s3|local, got {backend:?}"));
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
    if let Some(providers) = root.get("providers").filter(|p| !p.is_null()) {
        match providers.as_array() {
            Some(list) if !list.is_empty() => {
                for provider in list {
                    let ok = provider
                        .as_str()
                        .is_some_and(|p| matches!(p, "gcp" | "azure" | "aws" | "local"));
                    if !ok {
                        problems.push(format!("unknown provider: {provider:?}"));
                    }
                }
            }
            _ => problems.push("providers must be a non-empty list".to_string()),
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
        "project": "wisent-480400",
        "regions": ["us-central1"],
        "storage": {
            "backend": "gcs",
            "gcs": {"bucket": "stado"},
            "azure": {"account": "", "container": "wisent-compute"},
            "s3": {"bucket": "", "region": "us-east-1"},
            "local": {"path": "~/.stado/local-storage"},
        },
        "providers": ["gcp"],
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
        assert_eq!(get_in(&data, "a.b"), Some(&Value::String("file-value".into())));
        assert_eq!(get_in(&data, "a.missing.deep"), None);
        assert_eq!(get_in(&data, "a.b.deeper"), None); // b is not an object

        let bad = write_temp_config(r#"["not", "an", "object"]"#);
        assert!(matches!(
            load_uncached(bad.path()),
            Err(ConfigError::NotAnObject(_))
        ));
        let broken = write_temp_config("{invalid json");
        assert!(matches!(load_uncached(broken.path()), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn resolve_prefers_env_then_file_then_default() {
        // Unique env var / dotted keys so no other test or real config file
        // can interfere regardless of test execution order.
        std::env::set_var("STADO_TEST_RESOLVE_ENV", "from-env");
        assert_eq!(resolve("STADO_TEST_RESOLVE_ENV", "zz.nope", "dflt"), "from-env");
        // Empty-string env counts as unset.
        std::env::set_var("STADO_TEST_RESOLVE_ENV", "");
        assert_eq!(resolve("STADO_TEST_RESOLVE_ENV", "zz.nope", "dflt"), "dflt");
        std::env::remove_var("STADO_TEST_RESOLVE_ENV");
        assert_eq!(resolve("STADO_TEST_RESOLVE_MISSING", "zz.nope", "dflt"), "dflt");
    }

    #[test]
    fn resolve_list_parses_comma_env() {
        std::env::set_var("STADO_TEST_LIST_ENV", " a, b ,,c,");
        assert_eq!(resolve_list("STADO_TEST_LIST_ENV", "zz.nope", &["x"]), ["a", "b", "c"]);
        std::env::remove_var("STADO_TEST_LIST_ENV");
        assert_eq!(resolve_list("STADO_TEST_LIST_MISSING", "zz.nope", &["x", "y"]), ["x", "y"]);
    }

    #[test]
    fn validate_catches_structural_problems() {
        assert!(validate(&template()).is_empty());
        assert!(validate(&serde_json::json!({"storage": {"backend": "ftp"}}))
            .iter()
            .any(|p| p.contains("gcs|azure|s3|local")));
        assert!(validate(&serde_json::json!({"storage": {"backend": "gcs"}}))
            .iter()
            .any(|p| p.contains("storage.gcs.bucket")));
        assert!(validate(&serde_json::json!({"storage": {"backend": "s3"}}))
            .iter()
            .any(|p| p.contains("storage.s3.bucket")));
        assert!(validate(&serde_json::json!({"providers": []}))
            .iter()
            .any(|p| p.contains("non-empty list")));
        assert!(validate(&serde_json::json!({"providers": ["gcp", "dcloud"]}))
            .iter()
            .any(|p| p.contains("unknown provider")));
        assert!(validate(&serde_json::json!({"dashboard": {"port": 70000}}))
            .iter()
            .any(|p| p.contains("dashboard.port")));
        assert!(validate(&serde_json::json!({"dashboard": {"port": "8765"}}))
            .iter()
            .any(|p| p.contains("dashboard.port")));
        // Top-level "bucket" satisfies the gcs backend requirement.
        assert!(validate(&serde_json::json!({
            "storage": {"backend": "gcs"}, "bucket": "stado"
        }))
        .is_empty());
    }
}
