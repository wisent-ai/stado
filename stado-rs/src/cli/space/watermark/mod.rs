//! `stado space watermark TARGET`: read or set the memory declaration a host
//! is measured against.
//!
//! The space capability owns a host's declared space, and memory is a
//! watermark of it. With no write flag this prints the declaration in force,
//! whether the host declares one at all, and whether that declaration repairs
//! anything; with `--policy` it applies one declared policy whole; with the
//! `--memory-*` flags it edits fields. Every write rewrites
//! `targets[].memory_reclaim` through the canonical registry's own
//! compare-and-swap, validating the WHOLE document before the write, so an
//! incoherent declaration is refused with its own sentence and never reaches
//! the fleet.

use serde_json::{json, Value};

use super::{print_json, CmdError};
use crate::providers::local::host_memory::schema::MemoryReclaimPolicy;

mod args;
mod declared;
mod fields;

pub use args::WatermarkArgs;

/// `space watermark` body.
pub async fn dispatch(args: WatermarkArgs) -> Result<(), CmdError> {
    if args.authorize_graphical_session && args.policy.is_none() {
        return Err(CmdError::usage(
            "--authorize-graphical-session authorizes the graphical_session repair of a policy \
             named by --policy; on its own it authorizes nothing",
        )
        .stating(crate::primitives::failure::FailureCode::Refused)
        .machine_readable(args.json));
    }
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
        return declared::print_read(
            &args.target,
            entry.get("memory_reclaim").cloned(),
            args.json,
        );
    }

    let mut policy = match &args.policy {
        Some(name) => declared::declared_policy_for(name, entry, &args)?,
        None => match entry.get("memory_reclaim") {
            Some(existing) if existing.is_object() => existing.clone(),
            _ => {
                let mut seeded = serde_json::to_value(MemoryReclaimPolicy::reporting_default())?;
                fields::strip_nulls(&mut seeded);
                seeded
            }
        },
    };
    let policy_fields = policy
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target memory_reclaim must be an object"))?;
    fields::apply_fields(policy_fields, &args);
    fields::apply_repairs(policy_fields, &args)?;
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
            "applied_policy": args.policy,
            "memory_reclaim": policy,
        }));
    }
    match &args.policy {
        Some(name) => println!(
            "{}: declared policy {name} written at generation {generation}",
            args.target
        ),
        None => println!(
            "{}: memory_reclaim written at generation {generation}",
            args.target
        ),
    }
    println!("{}", serde_json::to_string_pretty(&policy)?);
    Ok(())
}
