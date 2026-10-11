//! The object-API recovery's readers of this host: where its stores are and
//! how its object route is addressed, the launchd definition the recovery
//! installs, whether two definitions are the same, and whether the running
//! server's object boundary is ready.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// The value the catalog's host Stado process passes `option`. The object
/// API's bind address and port are read from that declaration, so the
/// recovery probes, the lsof owner check and the definition it installs
/// address what the catalog runs, and no port is written here.
fn declared_option(option: &str) -> Result<String, String> {
    let host = crate::deploy::service_catalog::host_process()?;
    host.args
        .iter()
        .position(|arg| arg == option)
        .and_then(|index| host.args.get(index + 1))
        .cloned()
        .ok_or_else(|| format!("the catalog's host Stado process declares no {option}"))
}

/// The loopback port the object API listens on, as the catalog declares it.
pub(super) fn object_api_port() -> Result<String, String> {
    declared_option("--port")
}

/// The address the object API binds, as the catalog declares it.
pub(super) fn object_api_bind() -> Result<String, String> {
    declared_option("--bind")
}

/// `~` and `~/…` against HOME, then made absolute and lexically normalised.
pub(super) fn absolute(value: &str, home: &Path) -> PathBuf {
    let expanded = match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if value == "~" => home.to_path_buf(),
        None => PathBuf::from(value),
    };
    let expanded = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().unwrap_or_default().join(expanded)
    };
    let mut out = PathBuf::from("/");
    for component in expanded.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            _ => {}
        }
    }
    out
}

/// [`absolute`] with every symbolic link in the existing prefix resolved,
/// as `realpath` does for a path whose tail may not exist yet.
pub(super) fn real(value: &str, home: &Path) -> PathBuf {
    let path = absolute(value, home);
    let mut prefix = path.clone();
    let mut tail = Vec::new();
    while !prefix.exists() {
        match (
            prefix.file_name().map(|name| name.to_os_string()),
            prefix.parent(),
        ) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                prefix = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut resolved = std::fs::canonicalize(&prefix).unwrap_or(prefix);
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// The home directory; an unset HOME is refused, since read as the empty path
/// it would put the stores, the token files and the unit's HOME under `/`.
pub(super) fn home() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|error| format!("object API recovery refused: HOME cannot be read: {error}"))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub(super) fn text<'a>(document: &'a Value, pointer: &str) -> &'a str {
    document
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The host config, or `None` when there is none; a file that exists but
/// cannot be read, or is not JSON, is a refusal naming the file and cause.
fn config(path: &Path) -> Result<Option<Value>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "object API recovery refused: {}: {error}",
                path.display()
            ))
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("object API recovery refused: {}: {error}", path.display()))
}

/// `STORE\tBACKUP_STORE\tOBJECT_URL\tNAMESPACE\tTOKEN_FILE\tLABEL\tRETIRED`:
/// the environment wins for the stores, then the config, then the managed
/// defaults. `LABEL` is the host Stado unit the compiled catalog declares, the
/// one launchd label recovery installs and restarts; `RETIRED` the
/// comma-separated labels whose role was its API listener, whose loaded route
/// recovery reads before that unit takes over from them.
pub(super) async fn paths(config_path: &Path) -> Result<String, String> {
    let home = home()?;
    let document = config(config_path)?.unwrap_or(Value::Null);
    let configured = |pointer: &str| {
        let value = text(&document, pointer);
        (!value.is_empty()).then(|| real(value, &home).display().to_string())
    };
    let store = env("WC_LOCAL_STORAGE_PATH")
        .or_else(|| configured("/storage/local/path"))
        .unwrap_or_else(|| home.join(".stado/local-storage").display().to_string());
    let backup = env("WC_BACKUP_LOCAL_STORAGE_PATH")
        .or_else(|| configured("/storage/backup/local/path"))
        .unwrap_or_else(|| home.join(".stado/local-backup").display().to_string());
    let or = |pointer: &str, default: String| {
        let value = text(&document, pointer);
        if value.is_empty() {
            default
        } else {
            value.to_string()
        }
    };
    let token = match text(&document, "/storage/stado/token_file") {
        "" => home.join(".stado/queue-object-api-token"),
        value => absolute(value, &home),
    };
    let host = crate::deploy::service_catalog::host_process()?;
    let label = host.unit.clone().unwrap_or_else(|| host.name.clone());
    let object_url = format!("http://{}:{}", object_api_bind()?, object_api_port()?);
    // The units on this host that serve the API under another label, found
    // from what they run.
    let target = crate::deploy::service::local_target().map_err(|error| error.to_string())?;
    let retired =
        crate::deploy::service::api_predecessors_on(&target, &crate::deploy::production_runner())
            .await
            .map_err(|error| format!("this host's units could not be read: {error}"))?
            .join(",");
    Ok([
        real(&store, &home).display().to_string(),
        real(&backup, &home).display().to_string(),
        or("/storage/stado/url", object_url),
        or(
            "/storage/stado/namespace",
            crate::config::QUEUE_OBJECT_NAMESPACE.to_string(),
        ),
        token.display().to_string(),
        label,
        retired,
    ]
    .join("\t"))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// The running server's own operator state says its object boundary is
/// ready and carries no error.
pub(super) fn boundary_ready(state: &Path) -> bool {
    let Some(document) = std::fs::read(state)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return false;
    };
    let boundary = document
        .pointer("/boundaries/object")
        .cloned()
        .unwrap_or(Value::Null);
    boundary.get("ready") == Some(&Value::Bool(true))
        && !boundary.get("last_error").is_some_and(truthy)
}
