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

/// The user's home: HOME, else the home the user database records. With
/// neither, Stado stops with the cause; a `~/` left unexpanded named a
/// directory called `~` under whatever directory Stado started from, and an
/// empty home put token files under `/`.
pub(crate) fn home_dir() -> PathBuf {
    std::env::home_dir().expect(
        "neither HOME nor the user database names a home directory, so `~` cannot be expanded",
    )
}

/// Expand a leading `~` / `~/` to the user's home (Python `os.path.expanduser`).
pub(crate) fn expand_tilde(entry: &str) -> PathBuf {
    if entry == "~" {
        return home_dir();
    }
    match entry.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(entry),
    }
}

/// Locate the config file: $STADO_CONFIG override first (must exist), then
/// the candidate list. None when no candidate exists and none is selected.
/// A selection that is not text, or that names a file that does not exist,
/// is refused: read as no selection, Stado ran on another file or on none
/// while the caller believed it ran on theirs.
pub fn find_config_file() -> Result<Option<PathBuf>, ConfigError> {
    let override_ = match std::env::var(FILE_ENV) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(error) => {
            return Err(ConfigError::Selected {
                variable: FILE_ENV,
                message: format!("cannot be read: {error}"),
            })
        }
    };
    let override_ = override_.trim();
    // Only a candidate that does not exist is passed over: one that cannot be
    // examined was too, and Stado then ran on the next config or none.
    let present = |candidate: &std::path::Path| match std::fs::metadata(candidate) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ConfigError::Invalid {
            path: candidate.to_path_buf(),
            message: format!("cannot be examined: {error}"),
        }),
    };
    if !override_.is_empty() {
        let candidate = expand_tilde(override_);
        if !present(&candidate)? {
            return Err(ConfigError::Selected {
                variable: FILE_ENV,
                message: format!("names {}, which does not exist", candidate.display()),
            });
        }
        return Ok(Some(candidate));
    }
    for entry in CANDIDATES {
        let candidate = expand_tilde(entry);
        if present(&candidate)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
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
    let path = find_config_file()?;
    let data = match &path {
        None => Map::new(),
        Some(path) => load_uncached(path)?,
    };
    // First writer wins under a race; both computed equivalent results.
    let _ = CACHE.set(Cache { path, data });
    Ok(&CACHE.get().expect("cache just initialized").data)
}

/// The config file as it is on disk now, bypassing the process-wide cache.
/// For the one declaration a long-running process must see without a
/// restart: a release publisher declared after the process loaded its
/// configuration (see `release_publisher_for_key`).
pub fn load_config_file_fresh() -> Result<Map<String, Value>, ConfigError> {
    match find_config_file()? {
        None => Ok(Map::new()),
        Some(path) => load_uncached(&path),
    }
}

/// The path of the loaded config file, or None when running file-less.
/// Mirrors Python `config_path()`: forces a load first.
pub fn config_path() -> Result<Option<PathBuf>, ConfigError> {
    load_config_file()?;
    Ok(CACHE.get().and_then(|cache| cache.path.clone()))
}
