//! The storage route a loaded job was given, or the one its plist would give
//! it next time, as twelve tab-separated fields (`-` for empty):
//!
//! primary backend, primary root, backup backend, backup root, served
//! backend, served root, legacy implicit backup (yes/no), environment
//! matches the staged definition (yes/no), pid, state, explicit backend,
//! explicit root.
//!
//! The legacy server served the configured backup whenever its client
//! profile selected `stado`; that promotion is made explicit so a healthy
//! read from the backup cannot certify the primary. For a loaded job the
//! server's own runtime state, when it names the same pid, is the served
//! route.

mod launchctl;

use std::collections::BTreeMap;
use std::path::Path;

use plist::Value as Plist;
use serde_json::Value;

use self::launchctl::from_launchctl;
use super::config::{home, real, text};
use super::definition::environment;

fn refuse(detail: &str) -> String {
    format!("object API recovery refused: {detail}")
}

/// A launchd definition's top-level dictionary; a definition that cannot be
/// read, does not parse or is not a dictionary is refused with its path and
/// cause, never inspected as an empty one.
fn definition(path: &Path) -> Result<plist::Dictionary, String> {
    Plist::from_file(path)
        .map_err(|error| refuse(&format!("{}: {error}", path.display())))?
        .into_dictionary()
        .ok_or_else(|| refuse(&format!("{} is not a launchd dictionary", path.display())))
}

fn from_plist(source: &Path) -> Result<BTreeMap<String, String>, String> {
    let document = definition(source)?;
    let declared = environment(document.get("EnvironmentVariables").cloned(), source)
        .map_err(|error| refuse(&error))?;
    let mut environment = BTreeMap::new();
    for (key, value) in declared {
        let value = value
            .into_string()
            .ok_or_else(|| refuse("invalid environment dictionary"))?;
        environment.insert(key, value);
    }
    Ok(environment)
}

fn canonical_backend(value: &str) -> String {
    if value == "stado-object" {
        "stado".into()
    } else {
        value.into()
    }
}

pub(super) struct Inspection<'a> {
    pub(super) mode: &'a str,
    pub(super) source: &'a Path,
    pub(super) default_config: &'a str,
    pub(super) expected: &'a Path,
    pub(super) runtime: &'a Path,
}

pub(super) fn inspect(request: &Inspection) -> Result<String, String> {
    // An unreadable expected definition compared as an empty one would make
    // every loaded unit "match" it, and recovery would leave a wrong unit in
    // place.
    let expected_document = definition(request.expected)?;
    let expected: BTreeMap<String, String> = environment(
        expected_document.get("EnvironmentVariables").cloned(),
        request.expected,
    )
    .map_err(|error| refuse(&error))?
    .into_iter()
    .filter_map(|(key, value)| value.into_string().map(|value| (key, value)))
    .collect();
    let (environment, state, pid) = if request.mode == "launchctl" {
        let text =
            std::fs::read_to_string(request.source).map_err(|error| refuse(&error.to_string()))?;
        from_launchctl(&text)
    } else {
        // A definition on disk holds no running state or pid; only launchctl
        // reports those.
        (
            from_plist(request.source)?,
            "(not read: definition file, not launchctl)".into(),
            "(not read: definition file, not launchctl)".into(),
        )
    };
    // The loaded unit's own HOME decides; this process's HOME stands in only
    // when the unit names none, and an unset one is refused by home().
    let home_path: std::path::PathBuf = match environment.get("HOME").filter(|v| !v.is_empty()) {
        Some(declared) => declared.into(),
        None => home()?,
    };
    let expand = |value: &str| -> Result<String, String> {
        if value.is_empty() {
            Ok(String::new())
        } else {
            Ok(real(value, &home_path)?.display().to_string())
        }
    };
    // The unit's own STADO_CONFIG decides; the caller's default stands only
    // for a unit that names none.
    let config_path = match environment.get("STADO_CONFIG").filter(|v| !v.is_empty()) {
        Some(declared) => declared.as_str(),
        None => request.default_config,
    };
    let configuration: Value = std::fs::read(expand(config_path)?)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| refuse(&format!("cannot read loaded host config: {error}")))?;
    // A setting the unit's environment names decides; the loaded host config
    // answers for one the unit leaves out.
    let resolve = |name: &str, pointer: &str| match environment.get(name).filter(|v| !v.is_empty())
    {
        Some(declared) => declared.clone(),
        None => match text(&configuration, pointer) {
            Some(value) => value.to_string(),
            None => String::new(),
        },
    };
    let primary_backend = canonical_backend(&resolve("WC_STORAGE_BACKEND", "/storage/backend"));
    let primary_setting = resolve("WC_LOCAL_STORAGE_PATH", "/storage/local/path");
    let default_primary = home_path
        .join(crate::config_file::LOCAL_STORAGE_ROOT)
        .display()
        .to_string();
    let primary_root = expand(if primary_setting.is_empty() {
        &default_primary
    } else {
        &primary_setting
    })?;
    let backup_backend = canonical_backend(&resolve(
        "WC_BACKUP_STORAGE_BACKEND",
        "/storage/backup/backend",
    ));
    let backup_root = expand(&resolve(
        "WC_BACKUP_LOCAL_STORAGE_PATH",
        "/storage/backup/local/path",
    ))?;
    let mut legacy = primary_backend == "stado";
    let (mut served_backend, mut served_root) =
        if primary_backend.is_empty() || primary_backend == "local" {
            ("local".to_string(), primary_root.clone())
        } else if legacy {
            let root = if backup_backend == "local" {
                backup_root.clone()
            } else {
                String::new()
            };
            (backup_backend.clone(), root)
        } else {
            (primary_backend.clone(), String::new())
        };
    if request.mode == "launchctl" {
        // Only a missing runtime record means the process reported none; one
        // that cannot be read or parsed is refused, since read as none the
        // route was judged from config rather than from what the process
        // said it serves.
        let runtime: Option<Value> = match std::fs::read(request.runtime) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|error| {
                refuse(&format!(
                    "runtime identity {} is not JSON: {error}",
                    request.runtime.display()
                ))
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(refuse(&format!(
                    "runtime identity {} cannot be read: {error}",
                    request.runtime.display()
                )))
            }
        };
        match runtime
            .as_ref()
            .and_then(|runtime| runtime.get("storage"))
            .filter(|value| value.is_object())
        {
            Some(identity) => {
                let reported_pid = match identity.get("pid") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Null) | None => {
                        return Err(refuse("runtime identity names no pid"))
                    }
                    Some(other) => other.to_string(),
                };
                if reported_pid != pid {
                    return Err(refuse("runtime identity changed during inspection"));
                }
                served_backend = match text(identity, "/backend") {
                    Some(backend) => backend.to_string(),
                    None => String::new(),
                };
                served_root = match text(identity, "/local_path") {
                    Some(root) => expand(root)?,
                    None => String::new(),
                };
                legacy = false;
            }
            None if legacy && runtime.is_none() => {
                return Err(refuse("legacy storage route is unavailable"))
            }
            None => {}
        }
    }
    let matches = expected
        .iter()
        .all(|(key, value)| environment.get(key) == Some(value));
    // What the unit itself names, shown as `-` when it names nothing.
    let explicit_backend = match environment.get("WC_STORAGE_BACKEND") {
        Some(declared) => declared.clone(),
        None => String::new(),
    };
    let explicit_root = match environment.get("WC_LOCAL_STORAGE_PATH") {
        Some(declared) => expand(declared)?,
        None => String::new(),
    };
    let yes = |flag: bool| if flag { "yes" } else { "no" }.to_string();
    let fields = [
        primary_backend,
        primary_root,
        backup_backend,
        backup_root,
        served_backend,
        served_root,
        yes(legacy),
        yes(matches),
        pid,
        state,
        explicit_backend,
        explicit_root,
    ];
    if fields
        .iter()
        .any(|field| field.contains(['\t', '\r', '\n']))
    {
        return Err(refuse("route contains control characters"));
    }
    Ok(fields
        .iter()
        .map(|field| if field.is_empty() { "-" } else { field })
        .collect::<Vec<_>>()
        .join("\t"))
}
