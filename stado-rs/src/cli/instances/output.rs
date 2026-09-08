//! Shared output helpers: the placeholder the unresolvable cells print, the
//! age and yes/no formatting the table renders through, the JSON echo, the
//! enumeration-failure lines and the exit status those failures force.

use std::collections::BTreeMap;

use chrono::Duration;
use serde_json::Value;

use crate::cli::CmdError;

/// Printed where a value could not be resolved from any source.
pub(super) const UNKNOWN: &str = "-";

/// A provider we could not enumerate is a hole in the inventory, so the
/// command exits non-zero even when everything it *could* see was fine.
/// Reporting "no orphans" for a cloud we never reached is the failure this
/// command exists to prevent.
pub(super) fn enumeration_result(errors: &BTreeMap<String, String>) -> Result<(), CmdError> {
    if errors.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = errors.keys().map(String::as_str).collect();
    Err(CmdError::click(format!(
        "could not enumerate provider(s): {}",
        names.join(", ")
    )))
}

pub(super) fn print_errors(errors: &BTreeMap<String, String>) {
    for (provider, message) in errors {
        println!("{provider}: ENUMERATION FAILED — {message}");
    }
}

pub(super) fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// `<h>h<m>m` from a provider-reported age in seconds. Hours, not days: the
/// quantity the operator is reasoning about is billed GPU-hours. `chrono`
/// carries the unit arithmetic.
pub(super) fn format_age(age_seconds: f64) -> String {
    let Some(total) = Duration::try_seconds(age_seconds as i64) else {
        return UNKNOWN.to_string();
    };
    let hours = total.num_hours();
    let rest = total - Duration::try_hours(hours).unwrap_or_else(Duration::zero);
    format!("{hours}h{}m", rest.num_minutes())
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`, as in
/// `cli/quota.rs::echo_json`.
pub(super) fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}
