//! What makes a `targets[].memory_reclaim` declaration coherent.
//!
//! Called from `crate::targets`'s registry-v2 validator, so a declaration
//! that does not hold together never reaches the canonical document: `stado
//! registry validate` and `stado registry push` refuse it, and so does every
//! programmatic writer, because all of them go through the one validator.
//!
//! Each refusal has its own sentence. A single "invalid memory policy" would
//! be the defect this fleet keeps paying for — a declaration whose refusal
//! does not say which half of it is wrong is a declaration an operator fixes
//! by guessing.

use serde_json::Value;

use super::constants;
use super::schema::{REPAIR_GRAPHICAL_SESSION, REPAIR_REAP_RECOVERY, REPAIR_RESTART_UNIT};

/// A refusal: where in the document, and what is wrong there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPolicyProblem {
    pub location: String,
    pub message: String,
}

fn problem(location: &str, message: &str) -> MemoryPolicyProblem {
    MemoryPolicyProblem {
        location: location.to_string(),
        message: message.to_string(),
    }
}

fn require_int(
    value: &Value,
    location: &str,
    minimum: i64,
    maximum: Option<i64>,
) -> Result<i64, MemoryPolicyProblem> {
    let int = value
        .as_i64()
        .ok_or_else(|| problem(location, "must be an integer"))?;
    if int < minimum || maximum.is_some_and(|max| int > max) {
        let upper = maximum.map_or(String::new(), |max| format!(" and <= {max}"));
        return Err(problem(location, &format!("must be >= {minimum}{upper}")));
    }
    Ok(int)
}

fn list_repr(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("'{item}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Validate one target's memory-reclaim declaration.
pub fn validate(value: &Value, location: &str) -> Result<(), MemoryPolicyProblem> {
    let map = value
        .as_object()
        .ok_or_else(|| problem(location, "must be an object"))?;
    const REQUIRED: [&str; 7] = [
        "check_interval_seconds",
        "high_swap_used_pct",
        "low_free_mb",
        "max_repairs_per_pass",
        "mode",
        "repairs",
        "target_free_mb",
    ];
    const OPTIONAL: [&str; 2] = ["max_pass_seconds", "refuse_placement"];
    let missing: Vec<&str> = REQUIRED
        .into_iter()
        .filter(|key| !map.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Err(problem(
            location,
            &format!(
                "must contain exactly {}, and may add {}",
                list_repr(&REQUIRED),
                list_repr(&OPTIONAL)
            ),
        ));
    }
    for key in map.keys() {
        if !REQUIRED.contains(&key.as_str()) && !OPTIONAL.contains(&key.as_str()) {
            return Err(problem(
                location,
                &format!(
                    "does not declare {key:?}; it declares {}",
                    list_repr(&REQUIRED)
                ),
            ));
        }
    }
    if !matches!(map["mode"].as_str(), Some("off" | "report" | "enforce")) {
        return Err(problem(
            &format!("{location}.mode"),
            "must be one of 'off', 'report', or 'enforce'",
        ));
    }
    require_int(
        &map["check_interval_seconds"],
        &format!("{location}.check_interval_seconds"),
        constants::MIN_CHECK_INTERVAL_SECONDS,
        Some(constants::MAX_CHECK_INTERVAL_SECONDS),
    )?;
    let low = require_int(
        &map["low_free_mb"],
        &format!("{location}.low_free_mb"),
        constants::MIN_LOW_FREE_MB,
        None,
    )?;
    let target = require_int(
        &map["target_free_mb"],
        &format!("{location}.target_free_mb"),
        constants::MIN_LOW_FREE_MB,
        None,
    )?;
    if target <= low {
        return Err(problem(
            &format!("{location}.target_free_mb"),
            "must be greater than low_free_mb",
        ));
    }
    require_int(
        &map["high_swap_used_pct"],
        &format!("{location}.high_swap_used_pct"),
        1,
        Some(constants::PERCENT),
    )?;
    require_int(
        &map["max_repairs_per_pass"],
        &format!("{location}.max_repairs_per_pass"),
        1,
        Some(constants::MAX_REPAIRS_CEILING),
    )?;
    if let Some(declared) = map.get("max_pass_seconds") {
        require_int(
            declared,
            &format!("{location}.max_pass_seconds"),
            constants::MIN_PASS_SECONDS,
            Some(constants::MAX_PASS_SECONDS),
        )?;
    }
    if let Some(declared) = map.get("refuse_placement") {
        if !declared.is_boolean() {
            return Err(problem(
                &format!("{location}.refuse_placement"),
                "must be a boolean",
            ));
        }
    }
    validate_repairs(
        map["mode"].as_str().unwrap_or_default(),
        &map["repairs"],
        location,
    )
}

fn validate_repairs(mode: &str, value: &Value, location: &str) -> Result<(), MemoryPolicyProblem> {
    let repairs_location = format!("{location}.repairs");
    let repairs = value
        .as_object()
        .ok_or_else(|| problem(&repairs_location, "must be an object"))?;
    for (name, declared) in repairs {
        let here = format!("{repairs_location}.{name}");
        if !super::vocabulary::is_declared(name) {
            return Err(problem(
                &repairs_location,
                &format!(
                    "names {name:?}, and {} declares {}",
                    super::vocabulary::DECLARATION_PATH,
                    list_repr(&super::vocabulary::declared_names())
                ),
            ));
        }
        let fields = declared
            .as_object()
            .ok_or_else(|| problem(&here, "must be an object"))?;
        if let Some(age) = fields.get("min_age_seconds") {
            require_int(age, &format!("{here}.min_age_seconds"), 1, None)?;
        }
        match name.as_str() {
            REPAIR_RESTART_UNIT => {
                let units = fields
                    .get("units")
                    .and_then(Value::as_array)
                    .ok_or_else(|| problem(&format!("{here}.units"), "must be an array"))?;
                if units.is_empty() {
                    return Err(problem(
                        &format!("{here}.units"),
                        "must name at least one unit; a restart repair that names none is a \
                         declaration with no effect",
                    ));
                }
                if units.iter().any(|unit| !unit.is_string()) {
                    return Err(problem(&format!("{here}.units"), "must be strings"));
                }
            }
            REPAIR_REAP_RECOVERY => {
                let recovery = fields
                    .get("recovery")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        problem(
                            &format!("{here}.recovery"),
                            "must name the declared recovery program to run",
                        )
                    })?;
                if super::repairs::recovery_program(recovery).is_none() {
                    return Err(problem(
                        &format!("{here}.recovery"),
                        &format!("names {recovery:?}, which this build ships no program for"),
                    ));
                }
            }
            REPAIR_GRAPHICAL_SESSION => {
                let processes = fields
                    .get("processes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| problem(&format!("{here}.processes"), "must be an array"))?;
                if processes.is_empty() {
                    return Err(problem(
                        &format!("{here}.processes"),
                        "must name at least one process; terminating a graphical session is \
                         never decided by this product",
                    ));
                }
                if processes.iter().any(|process| !process.is_string()) {
                    return Err(problem(&format!("{here}.processes"), "must be strings"));
                }
                if let Some(flag) = fields.get("allow_graphical_session") {
                    if !flag.is_boolean() {
                        return Err(problem(
                            &format!("{here}.allow_graphical_session"),
                            "must be a boolean",
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if mode == "enforce" && repairs.is_empty() {
        return Err(problem(
            &repairs_location,
            "must name at least one repair when mode is 'enforce'; an enforcing pass with no \
             declared repair reports pressure it is not permitted to act on",
        ));
    }
    Ok(())
}
