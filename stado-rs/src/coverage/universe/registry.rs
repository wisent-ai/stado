//! The Universe contract plus the static in-process factory registry that
//! stands in for Python's `stado.coverage_universes` entry points.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use serde_json::{Map, Value};

use super::{UniverseEntry, Verifier};
use crate::models::py_str_repr;
use crate::queue::submit::SubmitOptions;

/// Submitter-defined contract describing the expected batch (Python
/// `Universe` ABC).
pub trait Universe: Send + Sync {
    /// Stable identifier for state-file scoping. URL-safe.
    fn id(&self) -> &str;
    /// Every (group_key, command, expected_uri) tuple.
    fn iter_entries(&self) -> Vec<UniverseEntry>;
    /// Verifier used to check expected_uri for entries from this universe.
    fn verifier(&self) -> Box<dyn Verifier>;
    /// Forwarded to submit on retry (Python `submit_kwargs()`); override
    /// for provider/priority/etc. Default = submit defaults.
    fn submit_options(&self) -> SubmitOptions {
        SubmitOptions::default()
    }
}

/// Constructor for a Universe from CLI `--kv` kwargs. The `Err` string is
/// surfaced as the CLI error message (Python `TypeError` from a bad
/// constructor kwarg surfaces as a traceback; here it is a clean error).
pub type UniverseFactory =
    Box<dyn Fn(Map<String, Value>) -> Result<Box<dyn Universe>, String> + Send + Sync>;

static UNIVERSES: LazyLock<Mutex<BTreeMap<String, UniverseFactory>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Register a universe factory under `name` (the static-registry analog of
/// a Python `stado.coverage_universes` entry point). Later registrations
/// with the same name replace earlier ones.
pub fn register_universe(name: impl Into<String>, factory: UniverseFactory) {
    UNIVERSES
        .lock()
        .expect("universe registry poisoned")
        .insert(name.into(), factory);
}

/// Registered universe ids, sorted (Python `list_universes` /
/// `sorted(discover_universes())`).
pub fn registered_universe_names() -> Vec<String> {
    UNIVERSES
        .lock()
        .expect("universe registry poisoned")
        .keys()
        .cloned()
        .collect()
}

/// Python `list_universes`.
pub fn list_universes() -> Vec<String> {
    registered_universe_names()
}

/// The exact click UsageError message Python raises for an unknown
/// universe id: `unknown universe {id!r}. Registered: {sorted or '(none)'}`.
pub fn unknown_universe_message(universe_id: &str) -> String {
    let names = registered_universe_names();
    let registered = if names.is_empty() {
        "(none)".to_string()
    } else {
        let items: Vec<String> = names.iter().map(|n| py_str_repr(n)).collect();
        format!("[{}]", items.join(", "))
    };
    format!(
        "unknown universe {}. Registered: {registered}",
        py_str_repr(universe_id)
    )
}

/// Instantiate a registered universe from CLI kwargs (Python
/// `_build_universe`).
pub fn build_universe(
    universe_id: &str,
    kwargs: Map<String, Value>,
) -> Result<Box<dyn Universe>, String> {
    {
        let registry = UNIVERSES.lock().expect("universe registry poisoned");
        if let Some(factory) = registry.get(universe_id) {
            // Called under the lock: factories must not re-enter the
            // registry (they construct, they don't register).
            return factory(kwargs);
        }
    } // Drop the guard before unknown_universe_message re-locks.
    Err(unknown_universe_message(universe_id))
}
