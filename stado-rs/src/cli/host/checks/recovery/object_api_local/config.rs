//! The object-API recovery's readers of this host: where its stores are and
//! how its object route is addressed, the launchd definition the recovery
//! installs, whether two definitions are the same, and whether the running
//! server's object boundary is ready.

use std::path::{Component, Path, PathBuf};

use plist::{Dictionary, Value as Plist};
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
fn object_api_bind() -> Result<String, String> {
    declared_option("--bind")
}
const LAUNCHD_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

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

pub(super) fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
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

/// The host config, or `None` when it cannot be read at all; a readable
/// file that is not JSON is a refusal.
fn config(path: &Path) -> Result<Option<Value>, String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(None);
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
    let home = home();
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

pub(super) struct Definition<'a> {
    pub(super) label: &'a str,
    pub(super) program: &'a str,
    pub(super) store: &'a str,
    pub(super) backup_store: &'a str,
    pub(super) account: &'a str,
    pub(super) log: &'a str,
    pub(super) config: &'a str,
}

/// The installed definition with the executable and required environment
/// recovery owns set; every other launchd option the shared service renderer
/// installed, resource limits included, is kept rather than rewritten.
pub(super) fn render(installed: &Path, staged: &Path, wanted: &Definition) -> Result<(), String> {
    let home = home().display().to_string();
    let mut document = Plist::from_file(installed)
        .ok()
        .and_then(Plist::into_dictionary)
        .unwrap_or_default();
    let mut environment = document
        .remove("EnvironmentVariables")
        .and_then(Plist::into_dictionary)
        .unwrap_or_default();
    document.remove("Program");
    let owned = [
        ("HOME", home.clone()),
        ("PATH", LAUNCHD_PATH.to_string()),
        ("STADO_CONFIG", wanted.config.to_string()),
        ("GNUPGHOME", format!("{home}/.gnupg")),
        (
            "SKARBIEC_VAULT_FILE",
            format!("{home}/.stado/skarbiec.vault.json"),
        ),
        (
            "WC_OBJECT_SKARBIEC_TOKEN_FILE",
            format!("{home}/.stado/stado-object-api-verifier-skarbiec-token"),
        ),
        (
            "WC_RELEASE_SKARBIEC_TOKEN_FILE",
            format!("{home}/.stado/stado-release-api-verifier-skarbiec-token"),
        ),
        ("WC_STORAGE_BACKEND", "local".to_string()),
        ("WC_LOCAL_STORAGE_PATH", wanted.store.to_string()),
        ("WC_BACKUP_STORAGE_BACKEND", "local".to_string()),
        (
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            wanted.backup_store.to_string(),
        ),
    ];
    for (key, value) in owned {
        environment.insert(key.to_string(), Plist::String(value));
    }
    let bind = object_api_bind()?;
    let port = object_api_port()?;
    let arguments = [
        wanted.program,
        "serve",
        "--api",
        "--bind",
        bind.as_str(),
        "--port",
        port.as_str(),
    ];
    let settings: [(&str, Plist); 8] = [
        ("Label", Plist::String(wanted.label.to_string())),
        (
            "ProgramArguments",
            Plist::Array(
                arguments
                    .iter()
                    .map(|arg| Plist::String(arg.to_string()))
                    .collect(),
            ),
        ),
        ("EnvironmentVariables", Plist::Dictionary(environment)),
        ("RunAtLoad", Plist::Boolean(true)),
        ("KeepAlive", Plist::Boolean(true)),
        ("UserName", Plist::String(wanted.account.to_string())),
        ("StandardOutPath", Plist::String(wanted.log.to_string())),
        ("StandardErrorPath", Plist::String(wanted.log.to_string())),
    ];
    for (key, value) in settings {
        document.insert(key.to_string(), value);
    }
    Plist::Dictionary(document)
        .to_file_xml(staged)
        .map_err(|error| format!("{}: {error}", staged.display()))
}

/// Two definitions are the same when both parse and hold equal values.
pub(super) fn same(left: &Path, right: &Path) -> bool {
    matches!((Plist::from_file(left), Plist::from_file(right)), (Ok(a), Ok(b)) if a == b)
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

pub(super) fn dictionary(value: Option<Plist>) -> Dictionary {
    value.and_then(Plist::into_dictionary).unwrap_or_default()
}
