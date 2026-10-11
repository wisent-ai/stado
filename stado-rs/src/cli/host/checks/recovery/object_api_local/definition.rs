//! The launchd definition the object-API recovery installs: rendered over
//! the installed one, keeping every option recovery does not own, and
//! compared with another by value.

use std::path::Path;

use plist::{Dictionary, Value as Plist};

use super::config::{home, object_api_bind, object_api_port};

const LAUNCHD_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
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

pub(super) fn dictionary(value: Option<Plist>) -> Dictionary {
    value.and_then(Plist::into_dictionary).unwrap_or_default()
}
