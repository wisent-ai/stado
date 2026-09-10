//! Editing one host's declaration field by field, which is what argv does
//! when it carries `--memory-*` flags instead of `--policy`.
//!
//! Kept for the case a declared policy does not cover: a host whose
//! watermarks or permitted repairs are genuinely its own. Everything here
//! writes into a policy document the caller already holds; the validation and
//! the compare-and-swap belong to the one writer in `mod.rs`.

use serde_json::{Map, Value};

use super::args::WatermarkArgs;
use crate::cli::CmdError;
use crate::providers::local::host_memory::schema::MemoryRepairPolicy;

/// Drop absent optional fields rather than writing them as `null`, which the
/// registry validator refuses.
pub(super) fn strip_nulls(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, field| !field.is_null());
        for field in map.values_mut() {
            strip_nulls(field);
        }
    }
}

/// Apply the scalar field flags onto a policy document.
pub(super) fn apply_fields(fields: &mut Map<String, Value>, args: &WatermarkArgs) {
    for (key, declared) in [
        ("mode", args.memory_mode.clone().map(Value::from)),
        ("low_free_mb", args.memory_low_free_mb.map(Value::from)),
        (
            "target_free_mb",
            args.memory_target_free_mb.map(Value::from),
        ),
        (
            "high_swap_used_pct",
            args.memory_high_swap_used_pct.map(Value::from),
        ),
        (
            "max_repairs_per_pass",
            args.memory_max_repairs_per_pass.map(Value::from),
        ),
        (
            "max_pass_seconds",
            args.memory_max_pass_seconds.map(Value::from),
        ),
        (
            "refuse_placement",
            args.memory_refuse_placement.map(Value::from),
        ),
    ] {
        if let Some(value) = declared {
            fields.insert(key.to_string(), value);
        }
    }
}

/// Apply the named repair flags onto a policy document.
pub(super) fn apply_repairs(
    policy: &mut Map<String, Value>,
    args: &WatermarkArgs,
) -> Result<(), CmdError> {
    let repairs = policy
        .entry("repairs")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("memory_reclaim.repairs must be an object"))?;
    for name in &args.memory_repair {
        repairs.entry(name.clone()).or_insert_with(|| {
            serde_json::to_value(MemoryRepairPolicy {
                units: Vec::new(),
                processes: Vec::new(),
                recovery: None,
                min_age_seconds: None,
                allow_graphical_session: false,
            })
            .unwrap_or(Value::Object(Map::new()))
        });
    }
    let mut field = |repair: &str, key: &str, value: Value| -> Result<(), CmdError> {
        let entry = repairs.get_mut(repair).ok_or_else(|| {
            CmdError::click(format!(
                "--memory-repair {repair} is required before its own flags can be set"
            ))
        })?;
        entry
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!("memory_reclaim.repairs.{repair} must be an object"))
            })?
            .insert(key.to_string(), value);
        Ok(())
    };
    if !args.memory_repair_unit.is_empty() {
        field(
            "restart_unit",
            "units",
            Value::Array(
                args.memory_repair_unit
                    .iter()
                    .cloned()
                    .map(Value::from)
                    .collect(),
            ),
        )?;
    }
    if !args.memory_repair_process.is_empty() {
        field(
            "graphical_session",
            "processes",
            Value::Array(
                args.memory_repair_process
                    .iter()
                    .cloned()
                    .map(Value::from)
                    .collect(),
            ),
        )?;
    }
    if let Some(recovery) = &args.memory_repair_recovery {
        field("reap_recovery", "recovery", Value::from(recovery.clone()))?;
    }
    if let Some(allow) = args.memory_allow_graphical_session {
        field(
            "graphical_session",
            "allow_graphical_session",
            Value::from(allow),
        )?;
    }
    Ok(())
}
