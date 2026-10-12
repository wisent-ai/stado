//! `stado product compiler-cache status|ensure|remove [--json]`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{declaration, ensure, remove, status};
use crate::common::emit;

/// Where the declared compiler cache stands on this machine.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum CacheState {
    Absent,
    Ready,
    OtherVersion,
    Unreadable,
}

/// The `stado product compiler-cache` operations.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Operation {
    Status,
    Ensure,
    Remove,
}

/// `stado product compiler-cache status|ensure|remove [--json]`.
pub fn run(name: &str, json_output: bool, home: &Path) -> Result<i32> {
    let operation = Operation::deserialize(Value::String(name.to_owned())).with_context(|| {
        format!("unknown compiler-cache operation {name}; use status, ensure or remove")
    })?;
    let report = match operation {
        Operation::Status => status(home)?,
        Operation::Ensure => {
            let declaration = declaration()?;
            let wrapper = ensure(home)?;
            let mut report = wrapper.report(&declaration);
            report["state"] = json!(CacheState::Ready);
            report
        }
        Operation::Remove => remove(home)?,
    };
    if json_output {
        emit(&report)?;
    } else {
        let field = |name: &str| match &report[name] {
            Value::String(text) => text.clone(),
            Value::Null => "-".to_owned(),
            other => other.to_string(),
        };
        println!(
            "{} {} at {} (declared {})",
            field("tool"),
            field("state"),
            field("path"),
            field("declared_version")
        );
        if let Value::String(error) = &report["error"] {
            eprintln!("{error}");
        }
    }
    // A removal succeeds by removing; the others when the declared version answers.
    let succeeded = operation == Operation::Remove
        || CacheState::deserialize(&report["state"]).is_ok_and(|state| state == CacheState::Ready);
    Ok(i32::from(!succeeded))
}
