//! `stado space watermark TARGET`: read or set the memory declaration a host
//! is measured against.
//!
//! The space capability owns a host's declared space, and memory is a
//! watermark of it. With no write flag this prints the declaration in force
//! and says whether the host declares one at all; with one or more write
//! flags it rewrites `targets[].memory_reclaim` through the canonical
//! registry's own compare-and-swap, validating the WHOLE document before the
//! write, so an incoherent declaration is refused with its own sentence and
//! never reaches the fleet.

use clap::Args;
use serde_json::{json, Map, Value};

use super::{print_json, CmdError};
use crate::providers::local::host_memory::schema::{MemoryReclaimPolicy, MemoryRepairPolicy};

/// Everything `space watermark` accepts.
#[derive(Args)]
pub struct WatermarkArgs {
    pub target: String,
    /// `off`, `report` or `enforce`; only `enforce` repairs anything.
    #[arg(long = "memory-mode")]
    pub memory_mode: Option<String>,
    /// Available memory below this many MiB is pressure.
    #[arg(long = "memory-low-free-mb")]
    pub memory_low_free_mb: Option<i64>,
    /// A pass stops as soon as this many MiB are available.
    #[arg(long = "memory-target-free-mb")]
    pub memory_target_free_mb: Option<i64>,
    /// Swap utilisation at or above this percentage is pressure on its own.
    #[arg(long = "memory-high-swap-used-pct")]
    pub memory_high_swap_used_pct: Option<i64>,
    /// How many repairs one pass may perform.
    #[arg(long = "memory-max-repairs-per-pass")]
    pub memory_max_repairs_per_pass: Option<i64>,
    /// Seconds one pass may spend.
    #[arg(long = "memory-max-pass-seconds")]
    pub memory_max_pass_seconds: Option<i64>,
    /// Permit one declared repair by name; repeat to permit several.
    #[arg(long = "memory-repair")]
    pub memory_repair: Vec<String>,
    /// A unit `restart_unit` may restart; repeat.
    #[arg(long = "memory-repair-unit")]
    pub memory_repair_unit: Vec<String>,
    /// A process `graphical_session` may end; repeat.
    #[arg(long = "memory-repair-process")]
    pub memory_repair_process: Vec<String>,
    /// The program `reap_recovery` runs.
    #[arg(long = "memory-repair-recovery")]
    pub memory_repair_recovery: Option<String>,
    /// Authorize `graphical_session` to end the processes it names.
    #[arg(long = "memory-allow-graphical-session", num_args = 1)]
    pub memory_allow_graphical_session: Option<bool>,
    /// Publish this host as not accepting jobs while it is over its watermark.
    #[arg(long = "memory-refuse-placement", num_args = 1)]
    pub memory_refuse_placement: Option<bool>,
    #[arg(long)]
    pub json: bool,
}

impl WatermarkArgs {
    /// Whether argv asked for a read rather than a write.
    fn is_read_only(&self) -> bool {
        self.memory_mode.is_none()
            && self.memory_low_free_mb.is_none()
            && self.memory_target_free_mb.is_none()
            && self.memory_high_swap_used_pct.is_none()
            && self.memory_max_repairs_per_pass.is_none()
            && self.memory_max_pass_seconds.is_none()
            && self.memory_repair.is_empty()
            && self.memory_repair_unit.is_empty()
            && self.memory_repair_process.is_empty()
            && self.memory_repair_recovery.is_none()
            && self.memory_allow_graphical_session.is_none()
            && self.memory_refuse_placement.is_none()
    }
}

fn strip_nulls(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, field| !field.is_null());
        for field in map.values_mut() {
            strip_nulls(field);
        }
    }
}

/// Apply the named repair flags onto a policy document.
fn apply_repairs(policy: &mut Map<String, Value>, args: &WatermarkArgs) -> Result<(), CmdError> {
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

/// `space watermark` body.
pub async fn dispatch(args: WatermarkArgs) -> Result<(), CmdError> {
    let store = crate::targets::RegistryStore::open().await?;
    let current = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("canonical registry generation unavailable"))?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(args.target.as_str()))
        .ok_or_else(|| CmdError::click(format!("target not in registry: {}", args.target)))?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;

    if args.is_read_only() {
        let declared = entry.get("memory_reclaim").cloned();
        if args.json {
            return print_json(&json!({
                "target": args.target,
                "declared": declared.is_some(),
                "memory_reclaim": declared,
            }));
        }
        match declared {
            Some(policy) => println!("{}", serde_json::to_string_pretty(&policy)?),
            None => println!(
                "{}: declares no memory_reclaim policy; it is measured against the reporting \
                 default, which reports and repairs nothing",
                args.target
            ),
        }
        return Ok(());
    }

    let mut policy = match entry.get("memory_reclaim") {
        Some(existing) if existing.is_object() => existing.clone(),
        _ => {
            let mut seeded = serde_json::to_value(MemoryReclaimPolicy::reporting_default())?;
            strip_nulls(&mut seeded);
            seeded
        }
    };
    let fields = policy
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target memory_reclaim must be an object"))?;
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
    apply_repairs(fields, &args)?;
    entry.insert("memory_reclaim".to_string(), policy.clone());

    // The WHOLE document, not the field: a declaration is only valid inside
    // the registry that carries it, and every writer in this product goes
    // through the one validator so a refusal reads the same wherever it came
    // from.
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if args.json {
        return print_json(&json!({
            "target": args.target,
            "generation": generation,
            "memory_reclaim": policy,
        }));
    }
    println!(
        "{}: memory_reclaim written at generation {generation}",
        args.target
    );
    println!("{}", serde_json::to_string_pretty(&policy)?);
    Ok(())
}
