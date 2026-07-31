//! Planning and validation for `stado-migrate coordinator`.
//!
//! Pure functions over the canonical registry document: no storage, no
//! network, no process state. The execution side (`run.rs`) consumes the
//! plan step by step; tests live in `tests.rs`.

use serde_json::Value;

/// One validated migration: source and target identities plus the ordered,
/// human-readable step list that `run` executes in sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    pub from_name: String,
    pub from_host: Option<String>,
    pub to_name: String,
    pub to_host: String,
    pub move_local_storage: bool,
    pub steps: Vec<String>,
}

/// The only runtime a migratable coordinator daemon may declare.
const DAEMON_RUNTIME: &str = "daemon";
/// Storage backend value that keeps the queue on one machine's own disk.
const LOCAL_BACKEND: &str = "local";

fn coordinators(document: &Value) -> Result<&Vec<Value>, String> {
    document
        .get("coordinators")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry document has no coordinators array".to_string())
}

fn entry_name(entry: &Value) -> Option<&str> {
    entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

fn is_active(entry: &Value) -> bool {
    entry.get("active").and_then(Value::as_bool) == Some(true)
}

fn host_of(entry: &Value) -> Option<String> {
    entry
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

fn find_named<'a>(entries: &'a [Value], name: &str) -> Result<&'a Value, String> {
    entries
        .iter()
        .find(|entry| entry_name(entry) == Some(name))
        .ok_or_else(|| format!("coordinator '{name}' not found in registry"))
}

/// Resolve the migration source: the explicit `--from`, or the single entry
/// marked active. Zero or several active entries make the source ambiguous
/// and are refused, matching `coordinator::resolve_coordinator`.
fn resolve_from<'a>(entries: &'a [Value], from: Option<&str>) -> Result<&'a Value, String> {
    if let Some(name) = from {
        return find_named(entries, name);
    }
    let active: Vec<&Value> = entries.iter().filter(|entry| is_active(entry)).collect();
    match active.as_slice() {
        [only] => Ok(*only),
        [] => Err("no active coordinator in registry; pass --from NAME explicitly".to_string()),
        many => Err(format!(
            "multiple active coordinators ({}); pass --from NAME explicitly",
            many.iter()
                .filter_map(|entry| entry_name(entry))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Build the validated plan, or explain exactly why migration cannot start.
pub fn build(
    document: &Value,
    from: Option<&str>,
    to: &str,
    storage_backend: &str,
    move_local_storage: bool,
) -> Result<MigrationPlan, String> {
    let entries = coordinators(document)?;
    let from_entry = resolve_from(entries, from)?;
    let from_name = entry_name(from_entry).unwrap_or_default().to_string();
    if from_name == to {
        return Err(format!(
            "source and target are both '{to}'; nothing to migrate"
        ));
    }
    let to_entry = find_named(entries, to)?;
    if is_active(to_entry) {
        return Err(format!("coordinator '{to}' is already the active entry"));
    }
    let runtime = to_entry
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or("");
    if runtime != DAEMON_RUNTIME {
        return Err(format!(
            "coordinator '{to}' has runtime='{runtime}'; only runtime='{DAEMON_RUNTIME}' can take over the tick"
        ));
    }
    let to_host = host_of(to_entry).ok_or_else(|| {
        format!(
            "coordinator '{to}' has no host; a migration target needs an operator-reachable host"
        )
    })?;
    if storage_backend == LOCAL_BACKEND && !move_local_storage {
        return Err(
            "storage backend is device-local; rerun with --move-local-storage so the target receives the queue store, or point both configs at a shared store"
                .to_string(),
        );
    }
    let from_host = host_of(from_entry);
    let steps = vec![
        format!("preflight: install the Stado release binaries on {to_host}"),
        format!("stop the '{from_name}' coordinator service on its current host"),
        if move_local_storage {
            format!("copy the device-local queue store to {to_host}")
        } else {
            "queue store stays put (shared backend, no copy needed)".to_string()
        },
        format!("bootstrap coordinator '{to}' on {to_host} via stado bootstrap --local"),
        format!("flip active in the canonical registry: '{to}' on, '{from_name}' off"),
        format!("verify the registry reads back with '{to}' as the only active coordinator"),
    ];
    Ok(MigrationPlan {
        from_name,
        from_host,
        to_name: to.to_string(),
        to_host,
        move_local_storage,
        steps,
    })
}
