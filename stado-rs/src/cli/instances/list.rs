//! `instances list`: the operator-facing body — enumerate the selected
//! providers, then print the fleet as a table (orphans on top) or as JSON,
//! and exit non-zero when a provider could not be enumerated.

use serde_json::{json, Value};

use crate::cli::{table, CmdError};
use crate::queue::JobStorage;

use super::fleet::inventory;
use super::output::{echo_json, enumeration_result, format_age, print_errors, yes_no, UNKNOWN};
use super::{fleet_providers, InstancesListArgs};

pub(super) async fn list(args: &InstancesListArgs) -> Result<(), CmdError> {
    let providers = fleet_providers(args.provider.as_deref())?;
    let store = JobStorage::new().await?;
    let fleet = inventory(&store, &providers).await?;

    if args.json {
        let instances: Vec<Value> = fleet
            .rows
            .iter()
            .map(|row| {
                json!({
                    "reference": row.reference,
                    "provider": row.provider,
                    "accel": row.accel,
                    "age_seconds": row.age_seconds,
                    "orphan": row.is_orphan(),
                    "held_by": row.held_by,
                })
            })
            .collect();
        echo_json(&json!({
            "providers": providers,
            "instances": instances,
            "errors": fleet.errors,
        }));
    } else {
        let rows: Vec<Vec<String>> = fleet
            .rows
            .iter()
            .map(|row| {
                vec![
                    row.reference.clone(),
                    row.provider.clone(),
                    row.accel.clone(),
                    format_age(row.age_seconds),
                    yes_no(row.is_orphan()).to_string(),
                    if row.held_by.is_empty() {
                        UNKNOWN.to_string()
                    } else {
                        row.held_by.join(", ")
                    },
                ]
            })
            .collect();
        table::print(
            &["REFERENCE", "PROVIDER", "ACCEL", "AGE", "ORPHAN", "HELD BY"],
            &rows,
        );
        let orphans = fleet.rows.iter().filter(|row| row.is_orphan()).count();
        println!("\n{} live VM(s), {orphans} orphan(s).", fleet.rows.len());
        for provider in &providers {
            if !fleet.errors.contains_key(provider) && fleet.rows_for(provider).next().is_none() {
                println!("{provider}: reported no agent VMs.");
            }
        }
        print_errors(&fleet.errors);
    }
    enumeration_result(&fleet.errors)
}
