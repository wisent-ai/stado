//! `stado space watermark TARGET`: read or set the disk and memory
//! declarations a host is measured against.
//!
//! The space capability owns a host's declared space, and disk and memory
//! are its two watermarks. With no write flag this prints the declarations
//! in force, whether the host declares them at all, and whether the memory
//! one repairs anything; with `--policy` it applies one declared memory
//! policy whole; with the `--memory-*` flags it edits memory fields and with
//! the `--disk-*` flags disk fields. Every write rewrites
//! `targets[].memory_reclaim` or `targets[].disk_cleanup` through the
//! canonical registry's own compare-and-swap, validating the WHOLE document
//! before the write, so an incoherent declaration is refused with its own
//! sentence and never reaches the fleet.
//!
//! The disk flags exist because Stado Desktop could already write
//! `low_free_gb` and `target_free_gb` through `api/registry/policy` while
//! the command line could not, and the release coordinator's
//! `release_scratch_short` refusal names the low watermark as the reserve a
//! build must fit above: an operator told to lower it had no command to do
//! it with.

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
            entry.get("disk_cleanup").cloned(),
            args.json,
        );
    }

    // What this call writes, in the order it is reported: the disk
    // declaration when any `--disk-*` flag is present, the memory one when a
    // policy or any `--memory-*` flag is. Both land in one compare-and-swap.
    let mut written: Vec<(&str, Value)> = Vec::new();
    if args.edits_disk_fields() {
        if args.policy.is_some() {
            return Err(CmdError::usage(
                "--policy applies a declared memory policy; the --disk-* flags edit the disk \
                 declaration by hand, and a call that did both would leave a document neither \
                 of them describes",
            )
            .stating(crate::primitives::failure::FailureCode::Refused)
            .machine_readable(args.json));
        }
        let mut policy = match entry.get("disk_cleanup") {
            Some(existing) if existing.is_object() => existing.clone(),
            _ => {
                let mut seeded =
                    serde_json::to_value(crate::targets::DiskCleanupPolicy::reporting_default())?;
                fields::strip_nulls(&mut seeded);
                seeded
            }
        };
        let policy_fields = policy
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target disk_cleanup must be an object"))?;
        for (key, value) in args.disk_fields() {
            policy_fields.insert(key.to_string(), value);
        }
        entry.insert("disk_cleanup".to_string(), policy.clone());
        written.push(("disk_cleanup", policy));
    }
    if args.policy.is_some() || args.edits_memory_fields() {
        let mut policy = match &args.policy {
            Some(name) => declared::declared_policy_for(name, entry, &args)?,
            None => match entry.get("memory_reclaim") {
                Some(existing) if existing.is_object() => existing.clone(),
                _ => {
                    let mut seeded =
                        serde_json::to_value(MemoryReclaimPolicy::reporting_default())?;
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
        written.push(("memory_reclaim", policy));
    }

    // The WHOLE document, not the field: a declaration is only valid inside
    // the registry that carries it, and every writer in this product goes
    // through the one validator so a refusal reads the same wherever it came
    // from.
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if args.json {
        let mut receipt = json!({
            "target": args.target,
            "generation": generation,
            "applied_policy": args.policy,
        });
        for (declaration, policy) in written {
            receipt[declaration] = policy;
        }
        return print_json(&receipt);
    }
    for (declaration, policy) in written {
        match &args.policy {
            Some(name) => println!(
                "{}: declared policy {name} written at generation {generation}",
                args.target
            ),
            None => println!(
                "{}: {declaration} written at generation {generation}",
                args.target
            ),
        }
        println!("{}", serde_json::to_string_pretty(&policy)?);
    }
    Ok(())
}
