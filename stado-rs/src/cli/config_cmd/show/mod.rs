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

/// `config show`: the resolved values for the operator-facing keys.
pub(super) fn show() -> Result<(), CmdError> {
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

    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    let mut out = Map::new();
    out.insert(
        "file".into(),
        where_
            .map(|p| Value::from(p.display().to_string()))
            .unwrap_or(Value::Null),
    );
    out.insert("resolved".into(), Value::Object(resolved));
    println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
    Ok(())
}
