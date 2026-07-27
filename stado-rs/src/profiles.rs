//! Job profiles — named bundles of submit flags for recurring workflows.
//!
//! Port of `stado/profiles/__init__.py`. A profile is a JSON file under the
//! crate's `data/profiles/` directory (or under the operator-supplied
//! `$WC_PROFILES_DIR`). It declares the same fields that `stado submit`
//! takes as CLI flags — gpu_type, vram_gb, apt, pre_command, repo, etc. —
//! so a recurring workflow ("Z-Image LoRA training on ai-toolkit",
//! "lm-eval on vLLM", ...) can be invoked with a single `--profile NAME`
//! flag instead of a 20-line one-shot command.
//!
//! Discovery order (first hit wins):
//!
//!   1. $WC_PROFILES_DIR/<name>.json     operator-local profiles
//!   2. <crate data>/profiles/<name>.json   bundled with the crate
//!
//! CLI flags ALWAYS override profile values. The profile is a default
//! template, not a hard contract.
//!
//! Schema (every field optional) — see the Python module docstring or
//! `data/profiles/ai_toolkit_zimage.json` for a full example.

use std::path::PathBuf;

use serde_json::{Map, Value};

/// The directory the bundled profiles ship in (Python `PACKAGE_PROFILES_DIR`
/// = `stado/profiles/`).
pub fn package_profiles_dir() -> PathBuf {
    crate::data_dir().join("profiles")
}

/// Maps profile JSON keys to submit kwarg names. Keeps the JSON
/// user-friendly (e.g. `apt` instead of `apt_packages`) while the
/// downstream code stays explicit. Ordered exactly like the Python dict
/// (irrelevant to semantics, kept for diff-friendliness).
pub const PROFILE_KEY_TO_KWARG: [(&str, &str); 16] = [
    ("gpu_type", "gpu_type"),
    ("vram_gb", "vram_gb"),
    ("machine_type", "machine_type"),
    ("apt", "apt_packages"),
    ("pre_command", "pre_command"),
    ("repo", "repo"),
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
        ("repo_workdir", Value::from("")),
        ("repo_extras", Value::from("train")),
        ("output_uri", Value::from("")),
        ("verify_command", Value::from("")),
        ("exclusive", Value::from(false)),
        ("priority", Value::from(0)),
        ("preemptible", Value::from(false)),
        ("max_cost_per_hour_usd", Value::from(0.0)),
        ("provider", Value::from("gcp")),
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
            map.entry("name".to_string()).or_insert_with(|| Value::from(name));
            return Ok(map);
        }
    }
    let available = list_profiles().join(", ");
    let available = if available.is_empty() { "(none)".to_string() } else { available };
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
pub fn merge_into_kwargs(profile: &Map<String, Value>, cli: &Map<String, Value>) -> Map<String, Value> {
    let mut out = cli.clone();
    for (pkey, kwarg) in PROFILE_KEY_TO_KWARG {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn bundled_profile_loads_with_name_defaulted() {
        let profile = load_profile("ai_toolkit_zimage").unwrap();
        assert_eq!(profile["name"], Value::from("ai_toolkit_zimage"));
        assert_eq!(profile["gpu_type"], Value::from("nvidia-l4"));
        assert_eq!(profile["vram_gb"], Value::from(22));
    }

    #[test]
    fn list_profiles_includes_bundled() {
        assert!(list_profiles().contains(&"ai_toolkit_zimage".to_string()));
    }

    #[test]
    fn missing_profile_error_lists_search_path() {
        let err = load_profile("no-such-profile-xyz").unwrap_err();
        let msg = err.to_string();
        assert!(msg.starts_with("profile 'no-such-profile-xyz' not found. Searched: "), "{msg}");
        assert!(msg.contains("Available: ai_toolkit_zimage"), "{msg}");
    }

    #[test]
    fn merge_adopts_profile_only_when_cli_is_default() {
        let profile = map(json!({
            "gpu_type": "nvidia-l4",
            "vram_gb": 22,
            "apt": ["libgl1"],
            "repo_extras": "",
            "spot": true,
            "max_cost_per_hour": 1.5,
        }));
        let cli = map(json!({
            "gpu_type": "nvidia-tesla-t4",  // explicit — wins over the profile
            "vram_gb": 0,                    // default — profile wins
            "machine_type": "",
            "apt_packages": [],
            "pre_command": "",
            "repo": "",
            "repo_workdir": "",
            "repo_extras": "train",          // historical default — profile wins
            "output_uri": "",
            "verify_command": "",
            "exclusive": false,
            "priority": 0,
            "preemptible": false,
            "max_cost_per_hour_usd": 0.0,
            "provider": "gcp",
            "pin_to_provider": false,
        }));
        let merged = merge_into_kwargs(&profile, &cli);
        assert_eq!(merged["gpu_type"], Value::from("nvidia-tesla-t4"));
        assert_eq!(merged["vram_gb"], Value::from(22));
        assert_eq!(merged["apt_packages"], json!(["libgl1"]));
        assert_eq!(merged["repo_extras"], Value::from(""));
        assert_eq!(merged["preemptible"], Value::from(true));
        assert_eq!(merged["max_cost_per_hour_usd"], Value::from(1.5));
        // Absent from the profile — untouched.
        assert_eq!(merged["machine_type"], Value::from(""));
        assert_eq!(merged["provider"], Value::from("gcp"));
    }

    #[test]
    fn merge_compares_int_and_float_defaults_like_python() {
        // Python `0 == 0.0` is True: an int 0 from the CLI must not block
        // the profile's max_cost_per_hour.
        let profile = map(json!({"max_cost_per_hour": 2.5}));
        let cli = map(json!({"max_cost_per_hour_usd": 0}));
        let merged = merge_into_kwargs(&profile, &cli);
        assert_eq!(merged["max_cost_per_hour_usd"], Value::from(2.5));
    }
}
