//! The submit kwargs a profile merges into, and the typed readers that take
//! the merged map apart again. Both halves speak the Python kwarg names, so
//! the profile merge stays a pure map-to-map operation.

use serde_json::{Map, Value};

use crate::cli::submit::SubmitArgs;

/// The submit kwargs the CLI passes, as a JSON map keyed by the Python
/// kwarg names — the exact input `profiles.merge_into_kwargs` expects.
pub(super) fn cli_kwargs_json(
    args: &SubmitArgs,
    apt_list: &[String],
    spot: bool,
    any_provider: bool,
) -> Map<String, Value> {
    Map::from_iter([
        ("gpu_type".into(), Value::from(args.gpu_type.as_str())),
        ("vram_gb".into(), Value::from(args.vram_gb)),
        (
            "machine_type".into(),
            Value::from(args.machine_type.as_str()),
        ),
        (
            "apt_packages".into(),
            Value::Array(apt_list.iter().map(|p| Value::from(p.as_str())).collect()),
        ),
        ("pre_command".into(), Value::from(args.pre_command.as_str())),
        ("repo".into(), Value::from(args.repo.as_str())),
        ("repo_ref".into(), Value::from(args.repo_ref.as_str())),
        (
            "repo_workdir".into(),
            Value::from(args.repo_workdir.as_str()),
        ),
        ("repo_extras".into(), Value::from(args.repo_extras.as_str())),
        ("output_uri".into(), Value::from(args.output_uri.as_str())),
        ("verify_command".into(), Value::from(args.verify.as_str())),
        ("exclusive".into(), Value::from(args.exclusive)),
        ("priority".into(), Value::from(args.priority)),
        ("deadline_at".into(), Value::from(args.deadline_at.as_str())),
        ("preemptible".into(), Value::from(spot)),
        (
            "max_cost_per_hour_usd".into(),
            Value::from(args.max_cost_per_hour),
        ),
        ("provider".into(), Value::from(args.provider.as_str())),
        ("pin_to_provider".into(), Value::from(!any_provider)),
    ])
}

pub(super) fn get_str(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn get_i64(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key).and_then(Value::as_i64).unwrap_or_default()
}

pub(super) fn get_f64(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key).and_then(Value::as_f64).unwrap_or_default()
}

pub(super) fn get_bool(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or_default()
}

pub(super) fn get_str_list(map: &Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
