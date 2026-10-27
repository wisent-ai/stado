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
    let mut arguments = host.args.iter();
    arguments
        .find(|argument| *argument == option)
        .and_then(|_option| arguments.next())
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

/// `~`, `~/…` and any other relative path against `home`, then lexically
/// normalised. The recovery runs under launchd, whose working directory is
/// no place the operator chose, so a relative store, token or state path
/// means the one under the home the unit runs as; read against the working
/// directory it named a different place in each caller.
pub(super) fn absolute(value: &str, home: &Path) -> PathBuf {
    let relative = match value.strip_prefix("~/") {
        Some(rest) => rest,
        None if value == "~" => "",
        None => value,
    };
    let expanded = home.join(relative);
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
/// as `realpath` does for a path whose tail may not exist yet. A prefix that
/// exists and still cannot be resolved is the refusal, naming it.
pub(super) fn real(value: &str, home: &Path) -> Result<PathBuf, String> {
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
    let mut resolved = std::fs::canonicalize(&prefix).map_err(|error| {
        format!(
            "object API recovery refused: {} cannot be resolved: {error}",
            prefix.display()
        )
    })?;
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// The home every Stado file is kept under ([`crate::config_file::home`]:
/// HOME, else the user database); with neither the recovery refuses, since
/// read as the empty path it would put the stores, the token files and the
/// unit's HOME under `/`.
pub(super) fn home() -> Result<PathBuf, String> {
    crate::config_file::home().map_err(|cause| format!("object API recovery refused: {cause}"))
}

/// The environment's value for `name` when it sets one; an empty one sets
/// nothing, and one that is not UTF-8 is refused naming the variable.
fn env(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(value)) => Err(format!(
            "object API recovery refused: {name} is not UTF-8 ({value:?})"
        )),
    }
}

/// The text the document holds at `pointer`, `None` when it holds none or
/// an empty one: both mean the setting is left to the next source.
pub(super) fn text<'a>(document: &'a Value, pointer: &str) -> Option<&'a str> {
    document
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// The host config, or `Null` when there is none (every setting is then the
/// environment's or the managed one); a file that exists but cannot be read,
/// or is not JSON, is a refusal naming the file and cause.
fn config(path: &Path) -> Result<Value, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => {
            return Err(format!(
                "object API recovery refused: {}: {error}",
                path.display()
            ))
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("object API recovery refused: {}: {error}", path.display()))
}

/// The first of: the environment's `variable`, the config's `pointer`, and
/// the managed path under the home, each resolved as `realpath` would.
fn store(
    variable: &str,
    document: &Value,
    pointer: &str,
    managed: &str,
    home: &Path,
) -> Result<String, String> {
    let declared = match env(variable)? {
        Some(value) => value,
        None => match text(document, pointer) {
            Some(value) => value.to_string(),
            None => home.join(managed).display().to_string(),
        },
    };
    Ok(real(&declared, home)?.display().to_string())
}

/// `STORE\tBACKUP_STORE\tOBJECT_URL\tNAMESPACE\tTOKEN_FILE\tLABEL\tRETIRED`:
/// the environment wins for the stores, then the config, then the managed
/// paths. `LABEL` is the host Stado unit the compiled catalog declares (its
/// name when it declares no unit), the one launchd label recovery installs
/// and restarts; `RETIRED` the comma-separated labels whose role was its API
/// listener, whose loaded route recovery reads before that unit takes over.
pub(super) async fn paths(config_path: &Path) -> Result<String, String> {
    let home = home()?;
    let document = config(config_path)?;
    let store_path = store(
        "WC_LOCAL_STORAGE_PATH",
        &document,
        "/storage/local/path",
        ".stado/local-storage",
        &home,
    )?;
    let backup_path = store(
        "WC_BACKUP_LOCAL_STORAGE_PATH",
        &document,
        "/storage/backup/local/path",
        ".stado/local-backup",
        &home,
    )?;
    let object_url = match text(&document, "/storage/stado/url") {
        Some(url) => url.to_string(),
        None => format!("http://{}:{}", object_api_bind()?, object_api_port()?),
    };
    let namespace = match text(&document, "/storage/stado/namespace") {
        Some(namespace) => namespace.to_string(),
        None => crate::config::QUEUE_OBJECT_NAMESPACE.to_string(),
    };
    let token = match text(&document, "/storage/stado/token_file") {
        Some(value) => absolute(value, &home),
        None => home.join(".stado/queue-object-api-token"),
    };
    let host = crate::deploy::service_catalog::host_process()?;
    let label = match &host.unit {
        Some(unit) => unit.clone(),
        None => host.name.clone(),
    };
    // The units on this host that serve the API under another label, found
    // from what they run.
    let target = crate::deploy::service::local_target().map_err(|error| error.to_string())?;
    let retired =
        crate::deploy::service::api_predecessors_on(&target, &crate::deploy::production_runner())
            .await
            .map_err(|error| format!("this host's units could not be read: {error}"))?
            .join(",");
    Ok([
        store_path,
        backup_path,
        object_url,
        namespace,
        token.display().to_string(),
        label,
        retired,
    ]
    .join("\t"))
}

/// Whether a recorded `last_error` says something went wrong: anything but
/// `null`, `false`, an empty string, list or object, or a zero.
fn names_an_error(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n.is_normal()),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// Whether the running server's own operator state says its object boundary
/// is ready and carries no error. A server that has not written its state
/// yet is not ready; a state that cannot be read or is not JSON is the
/// refusal, naming the file.
pub(super) fn boundary_ready(state: &Path) -> Result<bool, String> {
    let bytes = match std::fs::read(state) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("{}: {error}", state.display())),
    };
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not JSON: {error}", state.display()))?;
    let Some(boundary) = document.pointer("/boundaries/object") else {
        return Ok(false);
    };
    Ok(boundary.get("ready") == Some(&Value::Bool(true))
        && !boundary.get("last_error").is_some_and(names_an_error))
}
