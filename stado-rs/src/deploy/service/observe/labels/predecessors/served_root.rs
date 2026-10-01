//! The local root a predecessor unit serves, resolved the way that unit's own
//! process resolves it: the storage variables in its environment, else the
//! `storage` section of the config file it starts with (`STADO_CONFIG`, then
//! the candidates under its `HOME`), else the product's defaults. A unit that
//! declares nothing is not thereby unproven: it serves what the same binary
//! reads from the same config, and the first-generation object API units were
//! written exactly that way.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The local backend word.
pub(super) const LOCAL_BACKEND: &str = "local";

/// Whether a process whose primary backend is `backend` serves the local
/// root its config names through its API. `local` serves it directly; the
/// client routes `stado` and `stado-object` address an object API, and the
/// API that very process runs serves `storage.local.path`, which is how an
/// operator's machine shares the fleet's one registry. Every other backend
/// serves no local root.
pub(super) fn serves_local_root(backend: &str) -> bool {
    matches!(backend, LOCAL_BACKEND | "stado" | "stado-object")
}

/// What one unit resolves its primary store to.
pub(super) struct ServedRoot {
    pub(super) backend: String,
    /// The root, canonical when it exists; empty when the backend is not local.
    pub(super) root: String,
    /// Where each half came from, for the refusal a mismatch prints.
    pub(super) source: &'static str,
}

/// Resolve a unit's served root from `variable`, which answers one variable
/// of its environment, falling back to `home` when it declares none.
pub(super) fn resolve(variable: &dyn Fn(&str) -> Option<String>, home: &Path) -> ServedRoot {
    let declared = |key: &str| variable(key).filter(|value| !value.is_empty());
    let home = declared("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.to_path_buf());
    let expand = |value: &str| {
        value
            .strip_prefix("~/")
            .map(|rest| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(value))
    };
    if let (Some(backend), Some(root)) = (
        declared("WC_STORAGE_BACKEND"),
        declared("WC_LOCAL_STORAGE_PATH"),
    ) {
        return ServedRoot {
            backend,
            root: canonical(&expand(&root)),
            source: "its environment",
        };
    }
    let configuration = configuration(declared("STADO_CONFIG").as_deref(), &home);
    let configured = |pointer: &str| {
        configuration
            .as_ref()
            .and_then(|document| document.pointer(pointer))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let backend = declared("WC_STORAGE_BACKEND")
        .or_else(|| configured("/storage/backend"))
        .unwrap_or_else(|| LOCAL_BACKEND.to_string());
    let root = declared("WC_LOCAL_STORAGE_PATH")
        .or_else(|| configured("/storage/local/path"))
        .unwrap_or_else(|| "~/.stado/local-storage".to_string());
    ServedRoot {
        backend,
        root: canonical(&expand(&root)),
        source: if configuration.is_some() {
            "its config file"
        } else {
            "the product's defaults"
        },
    }
}

/// The config document a unit starts with: `STADO_CONFIG` when it names one,
/// else the first candidate under its home that exists.
fn configuration(explicit: Option<&str>, home: &Path) -> Option<Value> {
    let read = |path: PathBuf| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    if let Some(path) = explicit {
        return read(PathBuf::from(path));
    }
    crate::config_file::CANDIDATES.iter().find_map(|candidate| {
        let path = candidate
            .strip_prefix("~/")
            .map(|rest| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(candidate));
        read(path)
    })
}

/// The canonical form of a path that exists, else the path as written.
pub(super) fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
