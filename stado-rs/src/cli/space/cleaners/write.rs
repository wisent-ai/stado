//! The one registry write behind `space cleaners declare` and
//! `space cleaners remove`.
//!
//! The whole document is validated before the compare-and-swap, exactly as
//! `space watermark` does, so an incoherent policy is refused with the
//! registry's own sentence and never reaches the fleet. A host that declared
//! nothing starts from the reporting default it was already measured against,
//! so arming one cleaner cannot silently invent watermarks nobody chose.

use serde_json::{json, Value};

use crate::cli::space::print_json;
use crate::cli::CmdError;

/// `None` withdraws the cleaner; `Some` declares it with those fields.
pub(super) async fn write_cleaner(
    target: &str,
    cleaner: &str,
    declaration: Option<Value>,
    json_output: bool,
) -> Result<(), CmdError> {
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
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;
    let mut policy = match entry.get("disk_cleanup") {
        Some(existing) if existing.is_object() => existing.clone(),
        _ => {
            let mut seeded =
                serde_json::to_value(crate::targets::DiskCleanupPolicy::reporting_default())?;
            if let Some(object) = seeded.as_object_mut() {
                object.retain(|_, value| !value.is_null());
            }
            seeded
        }
    };
    let cleaners = policy
        .get_mut("cleaners")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click("registry target disk_cleanup.cleaners must be an object")
        })?;
    match declaration {
        Some(Value::Object(fields)) => {
            let value = cleaners
                .entry(cleaner.to_string())
                .or_insert_with(|| json!({}));
            let object = value
                .as_object_mut()
                .ok_or_else(|| CmdError::click("declared cleaner must be an object"))?;
            object.extend(fields);
            if !object.contains_key("min_age_seconds") {
                let spec = crate::providers::local::disk_cleanup::catalogue::cleaner(cleaner)
                    .ok_or_else(|| CmdError::usage(format!("unknown cleaner: {cleaner}")))?;
                object.insert("min_age_seconds".into(), json!(spec.min_age_floor_seconds));
            }
        }
        Some(_) => return Err(CmdError::usage("cleaner declaration must be an object")),
        None => {
            if cleaners.remove(cleaner).is_none() {
                return Err(CmdError::click(format!(
                    "{target} declares no cleaner {cleaner}"
                )));
            }
        }
    }
    entry.insert("disk_cleanup".to_string(), policy.clone());
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if json_output {
        return print_json(&json!({
            "target": target,
            "cleaner": cleaner,
            "generation": generation,
            "disk_cleanup": policy,
        }));
    }
    println!("{target}: disk_cleanup written at generation {generation}");
    println!("{}", serde_json::to_string_pretty(&policy)?);
    Ok(())
}
