//! The plane's verbs: declaring a database, removing a declaration, and
//! granting or revoking a consumer's access to one.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::writes::{canonical_name, mutate_databases, report_mutation};

pub(super) fn declare(
    name: &str,
    engine: &str,
    scopes: &[String],
    consumers: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if !canonical_name(name) {
        return Err(CmdError::usage(
            "NAME must be lowercase letters, digits and dashes",
        ));
    }
    if !crate::config::DATABASE_API_ENGINES.contains(&engine) {
        return Err(CmdError::usage(format!(
            "engine must be one of {:?}",
            crate::config::DATABASE_API_ENGINES
        )));
    }
    let mut clean_scopes: Vec<String> = scopes.to_vec();
    if clean_scopes.is_empty() {
        clean_scopes.push("read".to_string());
    }
    for scope in &clean_scopes {
        if !crate::config::DATABASE_API_SCOPES.contains(&scope.as_str()) {
            return Err(CmdError::usage(format!(
                "scope {scope:?} is not one of {:?}",
                crate::config::DATABASE_API_SCOPES
            )));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut clean_consumers = Vec::new();
    for consumer in consumers {
        let consumer = consumer.trim();
        if !canonical_name(consumer) {
            return Err(CmdError::usage(format!(
                "consumer {consumer:?} must be lowercase letters, digits and dashes"
            )));
        }
        if seen.insert(consumer.to_string()) {
            clean_consumers.push(consumer.to_string());
        }
    }

    let declaration = json!({
        "engine": engine,
        "scopes": clean_scopes,
        "consumers": clean_consumers,
    });
    mutate_databases(|map| {
        map.insert(name.to_string(), declaration.clone());
        Ok(())
    })?;
    report_mutation(
        json_output,
        json!({
            "declared": name,
            "engine": engine,
            "scopes": clean_scopes,
            "consumers": clean_consumers,
            "item": format!("{name}-database"),
        }),
    )
}

pub(super) fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    mutate_databases(|map| {
        if map.remove(name).is_none() {
            return Err(format!("database {name:?} is not declared"));
        }
        Ok(())
    })?;
    report_mutation(json_output, json!({"removed": name}))
}

pub(super) fn change_consumers(
    name: &str,
    consumers: &[String],
    grant: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    for consumer in consumers {
        let consumer = consumer.trim();
        if !canonical_name(consumer) {
            return Err(CmdError::usage(format!(
                "consumer {consumer:?} must be lowercase letters, digits and dashes"
            )));
        }
    }
    let mut changed = Vec::new();
    let mut item = format!("{name}-database");
    mutate_databases(|map| {
        let declaration = map
            .get_mut(name)
            .ok_or_else(|| format!("database {name:?} is not declared"))?;
        if let Some(declared) = declaration.get("item").and_then(Value::as_str) {
            item = declared.to_string();
        }
        let list = declaration
            .as_object_mut()
            .ok_or_else(|| format!("database {name:?} is malformed"))?
            .entry("consumers")
            .or_insert_with(|| json!([]));
        let list = list
            .as_array_mut()
            .ok_or_else(|| format!("database {name:?}.consumers must be an array"))?;
        for consumer in consumers {
            let consumer = consumer.trim().to_string();
            let entry = Value::String(consumer.clone());
            if grant {
                if !list.contains(&entry) {
                    list.push(entry);
                    changed.push(consumer);
                }
            } else {
                if list.len() == 1 && list[0] == entry {
                    return Err(format!(
                        "cannot revoke the last consumer of {name:?}; remove the declaration instead"
                    ));
                }
                if let Some(position) = list.iter().position(|existing| existing == &entry) {
                    list.remove(position);
                    changed.push(consumer);
                }
            }
        }
        Ok(())
    })?;
    // Declaring a consumer grants nothing in Skarbiec: a grant there is per
    // item, so `oko` stood on `oko`'s consumer list for weeks while every
    // read of `oko-database` answered 403 (defect 133b75aa). A grant now
    // widens each named consumer's own Skarbiec grant to read the item, the
    // union path that keeps its bearer and every capability it holds.
    let settled = if grant {
        settle_reads(&item, consumers)
    } else {
        Vec::new()
    };
    let failed: Vec<String> = settled
        .iter()
        .filter_map(|row| row.get("error").and_then(Value::as_str).map(String::from))
        .collect();
    report_mutation(
        json_output,
        json!({
            "database": name,
            "granted": if grant { changed.clone() } else { Vec::<String>::new() },
            "revoked": if grant { Vec::<String>::new() } else { changed },
            "skarbiec": settled,
        }),
    )?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "the declaration changed, but Skarbiec still refuses: {}",
            failed.join("; ")
        )))
    }
}

/// Widen each consumer's Skarbiec grant to read `item`, with the consumer's
/// own bearer file `~/.stado/<consumer>-skarbiec-token`.
fn settle_reads(item: &str, consumers: &[String]) -> Vec<Value> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    consumers
        .iter()
        .map(|consumer| {
            let consumer = consumer.trim();
            let token_file = home.join(".stado").join(format!("{consumer}-skarbiec-token"));
            match crate::credential_store::grant::grant_field_reads(consumer, &token_file, item, &[]) {
                Ok(outcome) => json!({ "consumer": consumer, "item": item, "added": outcome.added }),
                Err(error) => json!({ "consumer": consumer, "item": item, "error": format!("{consumer}: {error}") }),
            }
        })
        .collect()
}
