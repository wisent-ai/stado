//! `config show`: the resolved values for the operator-facing keys.
//!
//! The map is built once, in one order, and printed once, so each component
//! fills its own stretch of it and is called here in the order the printed
//! document has always had: the deployment's identity here, then `storage`,
//! `deployment`, `skarbiec` and `placement`.

mod deployment;
mod placement;
mod skarbiec;
mod storage;

use serde_json::{Map, Value};

use crate::config;
use crate::config_file;

use crate::cli::CmdError;

/// Every operator-facing key with its resolved value.
fn resolved() -> Map<String, Value> {
    // Keys mirror cli.py exactly (lowercased constant names).
    let mut resolved = Map::new();
    resolved.insert("project".into(), Value::from(config::project()));
    resolved.insert("bucket".into(), Value::from(config::bucket()));
    resolved.insert("region".into(), Value::from(config::region()));
    resolved.insert(
        "regions".into(),
        Value::Array(
            config::regions()
                .iter()
                .map(|r| Value::from(r.as_str()))
                .collect(),
        ),
    );
    storage::insert(&mut resolved);
    deployment::insert(&mut resolved);
    skarbiec::insert(&mut resolved);
    placement::insert(&mut resolved);
    resolved
}

/// `config show [--json]`: the config file and the resolved value of every
/// operator-facing key, as `key: value` lines or as one JSON document.
pub(super) fn show(json: bool) -> Result<(), CmdError> {
    let file = match config_file::config_path().map_err(CmdError::from)? {
        Some(path) => Value::from(path.display().to_string()),
        // No config file exists: the document says so with null, which
        // `config init` turns into a path.
        None => Value::Null,
    };
    let resolved = resolved();
    if json {
        let mut out = Map::new();
        out.insert("file".into(), file);
        out.insert("resolved".into(), Value::Object(resolved));
        println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
        return Ok(());
    }
    let mut lines = Map::new();
    lines.insert("file".into(), file);
    lines.extend(resolved);
    crate::cli::print_answer(&Value::Object(lines), false)
}

/// `config get KEY`: one value as bare text (a list or object as JSON). An
/// operator-facing key is answered as it resolves (environment, file,
/// default); any other key is the dotted key `config set` writes, read back
/// from the config file as it is on disk, so every `config set K V` the
/// documentation shows has a `config get K` that answers. A resolved key with
/// no value, a key the file does not hold, and a file that cannot be read are
/// each refused by name, so a script reading one never proceeds with an empty
/// value.
pub(super) fn get(key: &str) -> Result<(), CmdError> {
    let resolved = resolved();
    let value = match resolved.get(key) {
        Some(Value::String(text)) if text.is_empty() => Value::Null,
        Some(value) => value.clone(),
        None => file_value(key, &resolved)?,
    };
    match value {
        Value::Null => {
            return Err(
                CmdError::click(format!("configuration resolves no value for {key}"))
                    .stating(crate::primitives::failure::FailureCode::NotFound),
            );
        }
        Value::String(text) => println!("{text}"),
        other => println!("{other}"),
    }
    Ok(())
}

/// The dotted `key` in the config file on disk, walked segment by segment as
/// `config set` walks it; refused naming the file when there is none or it is
/// not JSON, and naming the keys `config get` resolves when the file holds no
/// such key.
fn file_value(key: &str, resolved: &Map<String, Value>) -> Result<Value, CmdError> {
    let path = config_file::config_path()
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{key}: no config file exists; run: stado config init"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| CmdError::from(error).within(format!("read {}", path.display())))?;
    let document: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!("{} is not JSON: {error}", path.display()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut cursor = &document;
    for segment in key.split('.') {
        cursor = cursor.get(segment).ok_or_else(|| {
            let keys: Vec<&str> = resolved.keys().map(String::as_str).collect();
            CmdError::click(format!(
                "{} holds no {key}, and {key} is not one of the keys config get resolves ({})",
                path.display(),
                keys.join(", ")
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    }
    Ok(cursor.clone())
}
