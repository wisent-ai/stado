//! The launchd definition the object-API recovery installs: rendered over
//! the installed one, keeping every option recovery does not own, and
//! compared with another by value.

use std::path::Path;

use plist::{Dictionary, Value as Plist};

use super::config::{home, object_api_bind, object_api_port};
use crate::primitives::tool_path::SEARCH_PATH;

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
    let home = home()?.display().to_string();
    // Only an absent definition starts empty: one that exists but cannot be
    // read is refused, since rendering over it would drop the launchd options
    // this function promises to keep.
    let mut document = match std::fs::metadata(installed) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Dictionary::new(),
        _ => Plist::from_file(installed)
            .map_err(|error| {
                format!(
                    "object API recovery refused: {}: {error}",
                    installed.display()
                )
            })?
            .into_dictionary()
            .ok_or_else(|| {
                format!(
                    "object API recovery refused: {} is not a launchd dictionary",
                    installed.display()
                )
            })?,
    };
    let mut environment = environment(document.remove("EnvironmentVariables"), installed)
        .map_err(|error| format!("object API recovery refused: {error}"))?;
    document.remove("Program");
    let owned = [
        ("HOME", home.clone()),
        ("PATH", SEARCH_PATH.to_string()),
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
    let settings = [
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

/// Two definitions are the same when both exist and hold equal values; an
/// absent one differs from any other. One that exists but cannot be read or
/// parsed is refused with its path and cause: read as "different", it sent
/// recovery on to rewrite a unit nobody could read.
pub(super) fn same(left: &Path, right: &Path) -> Result<bool, String> {
    let (Some(left), Some(right)) = (parsed(left)?, parsed(right)?) else {
        return Ok(false);
    };
    Ok(left == right)
}

/// A definition as parsed, or none when it does not exist.
fn parsed(path: &Path) -> Result<Option<Plist>, String> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Plist::from_file(path)
            .map(Some)
            .map_err(|error| format!("object API recovery refused: {}: {error}", path.display())),
    }
}

/// A definition's environment: an absent one is empty, and one that is not a
/// dictionary is refused naming the definition, since reading or rendering
/// over it as empty would drop what it holds.
pub(super) fn environment(value: Option<Plist>, source: &Path) -> Result<Dictionary, String> {
    match value {
        None => Ok(Dictionary::new()),
        Some(value) => value.into_dictionary().ok_or_else(|| {
            format!(
                "the EnvironmentVariables of {} is not a dictionary",
                source.display()
            )
        }),
    }
}
