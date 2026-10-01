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
    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    let file = where_
        .map(|p| Value::from(p.display().to_string()))
        .unwrap_or(Value::Null);
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

/// `config get KEY`: one resolved value as bare text (a list or object as
/// JSON). A key that resolves to nothing is refused by name, so a script
/// reading it never proceeds with an empty value.
pub(super) fn get(key: &str) -> Result<(), CmdError> {
    match resolved().get(key) {
        Some(Value::String(text)) if !text.is_empty() => println!("{text}"),
        Some(Value::Null) | Some(Value::String(_)) | None => {
            return Err(CmdError::click(format!(
                "configuration resolves no value for {key}"
            )));
        }
        Some(other) => println!("{other}"),
    }
    Ok(())
}
