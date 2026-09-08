//! Where the config file lives, and the one process-wide load that answers for
//! it.
//!
//! `~` expansion, the $STADO_CONFIG override and the candidate search, the
//! [`std::sync::OnceLock`] every reader goes through, and the path that cache
//! recorded. A parse failure is not cached, so the next call retries.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value};

use super::{ConfigError, CANDIDATES, FILE_ENV};

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
