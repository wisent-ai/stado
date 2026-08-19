//! Job profiles — named bundles of submit flags for recurring workflows.
//!
//! Bundled profiles are embedded in the executable so installed binaries do
//! not depend on the build machine's `CARGO_MANIFEST_DIR`. Operators may add
//! or override profiles beside the installed binary or through
//! `$WC_PROFILES_DIR`.
//!
//! Discovery order (first hit wins):
//!
//!   1. $WC_PROFILES_DIR/<name>.json
//!   2. <installed executable directory>/profiles/<name>.json
//!   3. profiles embedded in the executable
//!
//! CLI flags ALWAYS override profile values. The profile is a default
//! template, not a hard contract.
//!
//! Schema (every field optional) — see the Python module docstring or
//! `data/profiles/ai_toolkit_zimage.json` for a full example.

use std::path::PathBuf;

use serde_json::{Map, Value};

const BUNDLED_PROFILES: &[(&str, &str)] = &[(
    "ai_toolkit_zimage",
    include_str!("../data/profiles/ai_toolkit_zimage.json"),
)];

/// Runtime-adjacent profile directory for operator-managed installations.
/// Bundled profiles themselves are embedded in [`BUNDLED_PROFILES`].
pub fn package_profiles_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("profiles")))
        .unwrap_or_else(|| PathBuf::from("profiles"))
}

/// Maps profile JSON keys to submit kwarg names. Keeps the JSON
/// user-friendly (e.g. `apt` instead of `apt_packages`) while the
/// downstream code stays explicit. Ordered exactly like the Python dict
/// (irrelevant to semantics, kept for diff-friendliness).
pub const PROFILE_KEY_TO_KWARG: &[(&str, &str)] = &[
    ("gpu_type", "gpu_type"),
    ("vram_gb", "vram_gb"),
    ("machine_type", "machine_type"),
    ("apt", "apt_packages"),
    ("pre_command", "pre_command"),
    ("repo", "repo"),
    ("repo_ref", "repo_ref"),
    ("repo_workdir", "repo_workdir"),
    ("repo_extras", "repo_extras"),
    ("output_uri", "output_uri"),
    ("verify", "verify_command"),
    ("exclusive", "exclusive"),
    ("priority", "priority"),
    ("spot", "preemptible"),
    ("max_cost_per_hour", "max_cost_per_hour_usd"),
    ("provider", "provider"),
    ("pin_provider", "pin_to_provider"),
];

/// The wisent-compute submit defaults a CLI kwarg is compared against in
/// [`merge_into_kwargs`]. `repo_extras: "train"` is a historical default —
/// keep. Ordered like the Python `DEFAULTS` dict.
fn kwarg_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("gpu_type", Value::from("")),
        ("vram_gb", Value::from(0)),
        ("machine_type", Value::from("")),
        ("apt_packages", Value::Array(vec![])),
        ("pre_command", Value::from("")),
        ("repo", Value::from("")),
        ("repo_ref", Value::from("")),
        ("repo_workdir", Value::from("")),
        ("repo_extras", Value::from("train")),
        ("output_uri", Value::from("")),
        ("verify_command", Value::from("")),
        ("exclusive", Value::from(false)),
        ("priority", Value::from(0)),
        ("preemptible", Value::from(false)),
        ("max_cost_per_hour_usd", Value::from(0.0)),
        ("provider", Value::from("")),
        ("pin_to_provider", Value::from(false)),
    ]
}

fn default_for(kwarg: &str) -> Option<Value> {
    kwarg_defaults()
        .into_iter()
        .find(|(key, _)| *key == kwarg)
        .map(|(_, value)| value)
}

/// Profile load failure. [`ProfileError::NotFound`] maps to Python
/// `FileNotFoundError`, [`ProfileError::Invalid`] to Python `ValueError`.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Profile search path. Operator override first, package second.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let override_ = std::env::var("WC_PROFILES_DIR").unwrap_or_default();
    let override_ = override_.trim();
    if !override_.is_empty() {
        dirs.push(PathBuf::from(override_));
    }
    dirs.push(package_profiles_dir());
    dirs
}

/// All profile names visible on the discovery path. De-duped, sorted
/// within each directory; first directory wins on name clashes (Python
/// `list_profiles`).
pub fn list_profiles() -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for dir in candidate_dirs() {
        if !dir.is_dir() {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.ends_with(".json"))
                    .map(|name| name[..name.len() - ".json".len()].to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        for name in names {
            if !seen.contains(&name) {
                seen.push(name);
            }
        }
    }
    for (name, _) in BUNDLED_PROFILES {
        if !seen.iter().any(|visible| visible == name) {
            seen.push((*name).to_string());
        }
    }
    seen
}

/// Python `type(data).__name__` for the non-object error message.
fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Return the profile JSON as an object. [`ProfileError::NotFound`] when
/// absent on the whole discovery path.
///
/// The first match on the discovery path wins, so operator-local profiles
/// in $WC_PROFILES_DIR override bundled ones with the same name.
pub fn load_profile(name: &str) -> Result<Map<String, Value>, ProfileError> {
    for dir in candidate_dirs() {
        let path = dir.join(format!("{name}.json"));
        if path.is_file() {
            let text = std::fs::read_to_string(&path)?;
            let data: Value = serde_json::from_str(&text)?;
            let mut map = match data {
                Value::Object(map) => map,
                other => {
                    return Err(ProfileError::Invalid(format!(
                        "profile {name}: expected JSON object, got {}",
                        python_type_name(&other)
                    )));
                }
            };
            map.entry("name".to_string())
                .or_insert_with(|| Value::from(name));
            return Ok(map);
        }
    }
    if let Some((_, text)) = BUNDLED_PROFILES
        .iter()
        .find(|(bundled_name, _)| *bundled_name == name)
    {
        let data: Value = serde_json::from_str(text)?;
        let mut map = match data {
            Value::Object(map) => map,
            other => {
                return Err(ProfileError::Invalid(format!(
                    "profile {name}: expected JSON object, got {}",
                    python_type_name(&other)
                )));
            }
        };
        map.entry("name".to_string())
            .or_insert_with(|| Value::from(name));
        return Ok(map);
    }
    let available = list_profiles().join(", ");
    let available = if available.is_empty() {
        "(none)".to_string()
    } else {
        available
    };
    Err(ProfileError::NotFound(format!(
        "profile '{name}' not found. Searched: {}. Available: {available}",
        candidate_dirs()
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Python `==` between a CLI value and a default. Numbers compare
/// numerically so JSON `0` equals `0.0` (Python `0 == 0.0` is True);
/// everything else compares structurally.
fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Return submit kwargs with profile values applied where the CLI didn't
/// pass an explicit value.
///
/// `cli` is the map of caller-supplied kwargs from `stado submit`, keyed by
/// the kwarg names in [`PROFILE_KEY_TO_KWARG`] values. A kwarg counts as
/// "explicit" when its value differs from the wisent-compute defaults —
/// empty string, 0, False, []. This means a user who passes
/// `--gpu-type nvidia-l4` overrides the profile's gpu_type; a user who
/// doesn't pass --gpu-type takes whatever the profile says (including
/// nothing).
pub fn merge_into_kwargs(
    profile: &Map<String, Value>,
    cli: &Map<String, Value>,
) -> Map<String, Value> {
    let mut out = cli.clone();
    for &(pkey, kwarg) in PROFILE_KEY_TO_KWARG {
        let Some(profile_value) = profile.get(pkey) else {
            continue;
        };
        let default = default_for(kwarg);
        let cli_val = cli.get(kwarg).cloned().or_else(|| default.clone());
        match (&cli_val, &default) {
            (Some(value), Some(default)) if py_eq(value, default) => {
                // CLI didn't override — adopt the profile value.
                out.insert(kwarg.to_string(), profile_value.clone());
            }
            _ => {}
        }
    }
    out
}

