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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use plist::Value as Plist;
use regex::Regex;
use serde_json::Value;

use super::config::{dictionary, home, real, text};

static ENVIRONMENT_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*environment = \{\s*$").expect("static"));
static BLOCK_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\}\s*$").expect("static"));
static ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([^=\s]+)\s+=>\s+(.*?)\s*$").expect("static"));
static STATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*state = (.*?)\s*$").expect("static"));
static PID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*pid = ([0-9]+)\s*$").expect("static"));

fn refuse(detail: &str) -> String {
    format!("object API recovery refused: {detail}")
}

/// `launchctl print` output: the job's environment block, its state and pid.
fn from_launchctl(text: &str) -> (BTreeMap<String, String>, String, String) {
    let (mut environment, mut state, mut pid) = (BTreeMap::new(), None, None);
    let mut inside = false;
    for line in text.lines() {
        if ENVIRONMENT_OPEN.is_match(line) {
            inside = true;
            continue;
        }
        if inside {
            if BLOCK_CLOSE.is_match(line) {
                inside = false;
            } else if let Some(found) = ENTRY.captures(line) {
                environment.insert(found[1].to_string(), found[2].to_string());
            }
            continue;
        }
        if let Some(found) = STATE.captures(line) {
            state.get_or_insert_with(|| found[1].to_string());
        }
        if let Some(found) = PID.captures(line) {
            pid.get_or_insert_with(|| found[1].to_string());
        }
    }
    (environment, state.unwrap_or_else(|| "-".into()), pid.unwrap_or_else(|| "-".into()))
}

fn from_plist(source: &Path) -> Result<BTreeMap<String, String>, String> {
    let document = dictionary(Plist::from_file(source).ok());
    let mut environment = BTreeMap::new();
    for (key, value) in dictionary(document.get("EnvironmentVariables").cloned()) {
        let value = value.into_string().ok_or_else(|| refuse("invalid environment dictionary"))?;
        environment.insert(key, value);
    }
    Ok(environment)
}

fn canonical_backend(value: &str) -> String {
    if value == "stado-object" { "stado".into() } else { value.into() }
}

pub(super) struct Inspection<'a> {
    pub(super) mode: &'a str,
    pub(super) source: &'a Path,
    pub(super) default_config: &'a str,
    pub(super) expected: &'a Path,
    pub(super) runtime: &'a Path,
}

pub(super) fn inspect(request: &Inspection) -> Result<String, String> {
    let expected: BTreeMap<String, String> = dictionary(Plist::from_file(request.expected).ok())
        .get("EnvironmentVariables")
        .cloned()
        .and_then(Plist::into_dictionary)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| value.into_string().map(|value| (key, value)))
        .collect();
    let (environment, state, pid) = if request.mode == "launchctl" {
        let text = std::fs::read_to_string(request.source).map_err(|error| refuse(&error.to_string()))?;
        from_launchctl(&text)
    } else {
        (from_plist(request.source)?, "-".into(), "-".into())
    };
    let default_home = home();
    let home_path = environment.get("HOME").filter(|v| !v.is_empty()).map(Into::into).unwrap_or(default_home);
    let expand = |value: &str| if value.is_empty() { String::new() } else { real(value, &home_path).display().to_string() };
    let config_path = environment.get("STADO_CONFIG").map(String::as_str).filter(|v| !v.is_empty()).unwrap_or(request.default_config);
    let configuration: Value = std::fs::read(expand(config_path))
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| refuse(&format!("cannot read loaded host config: {error}")))?;
    let resolve = |name: &str, pointer: &str| {
        environment.get(name).filter(|v| !v.is_empty()).cloned().unwrap_or_else(|| text(&configuration, pointer).to_string())
    };
    let primary_backend = canonical_backend(&resolve("WC_STORAGE_BACKEND", "/storage/backend"));
    let primary_setting = resolve("WC_LOCAL_STORAGE_PATH", "/storage/local/path");
    let default_primary = home_path.join(".stado/local-storage").display().to_string();
    let primary_root = expand(if primary_setting.is_empty() { &default_primary } else { &primary_setting });
    let backup_backend = canonical_backend(&resolve("WC_BACKUP_STORAGE_BACKEND", "/storage/backup/backend"));
    let backup_root = expand(&resolve("WC_BACKUP_LOCAL_STORAGE_PATH", "/storage/backup/local/path"));
    let mut legacy = primary_backend == "stado";
    let (mut served_backend, mut served_root) = if primary_backend.is_empty() || primary_backend == "local" {
        ("local".to_string(), primary_root.clone())
    } else if legacy {
        let root = if backup_backend == "local" { backup_root.clone() } else { String::new() };
        (backup_backend.clone(), root)
    } else {
        (primary_backend.clone(), String::new())
    };
    if request.mode == "launchctl" {
        let runtime: Option<Value> = std::fs::read(request.runtime).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match runtime.as_ref().and_then(|runtime| runtime.get("storage")).filter(|value| value.is_object()) {
            Some(identity) => {
                let reported_pid = match identity.get("pid") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Null) | None => "None".into(),
                    Some(other) => other.to_string(),
                };
                if reported_pid != pid {
                    return Err(refuse("runtime identity changed during inspection"));
                }
                served_backend = text(identity, "/backend").to_string();
                served_root = expand(text(identity, "/local_path"));
                legacy = false;
            }
            None if legacy && runtime.is_none() => return Err(refuse("legacy storage route is unavailable")),
            None => {}
        }
    }
    let matches = expected.iter().all(|(key, value)| environment.get(key) == Some(value));
    let explicit_backend = environment.get("WC_STORAGE_BACKEND").cloned().unwrap_or_default();
    let explicit_root = expand(environment.get("WC_LOCAL_STORAGE_PATH").map(String::as_str).unwrap_or(""));
    let yes = |flag: bool| if flag { "yes" } else { "no" }.to_string();
    let fields = [
        primary_backend, primary_root, backup_backend, backup_root, served_backend, served_root,
        yes(legacy), yes(matches), pid, state, explicit_backend, explicit_root,
    ];
    if fields.iter().any(|field| field.contains(['\t', '\r', '\n'])) {
        return Err(refuse("route contains control characters"));
    }
    Ok(fields.iter().map(|field| if field.is_empty() { "-" } else { field }).collect::<Vec<_>>().join("\t"))
}
