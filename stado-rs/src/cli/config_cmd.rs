//! `stado config SUB` — port of the `config` command in `stado/cli.py`:
//! positional string dispatch (NOT real subcommands) over
//! show | validate | init.

use serde_json::{Map, Value};

use crate::config;
use crate::config_file;

use super::CmdError;

pub fn run(sub: &str) -> Result<(), CmdError> {
    match sub {
        "init" => init(),
        "validate" => validate(),
        "show" => show(),
        other => Err(CmdError::click(format!(
            "unknown config subcommand: {other} (show|validate|init)"
        ))),
    }
}

/// `config init`: write the commented template to ~/.stado/config.json.
fn init() -> Result<(), CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let path = std::path::Path::new(&home).join(".stado").join("config.json");
    if path.exists() {
        return Err(CmdError::click(format!("config file already exists: {}", path.display())));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&config_file::template())?;
    std::fs::write(&path, format!("{body}\n"))?;
    println!("{}", path.display());
    Ok(())
}

/// `config validate`: structural check; problems print as `ERROR ...`
/// lines and exit 1 (Python `raise SystemExit(1)`).
fn validate() -> Result<(), CmdError> {
    let data = config_file::load_config_file().map_err(|exc| CmdError::click(exc.to_string()))?;
    let problems = config_file::validate(&Value::Object(data.clone()));
    if !problems.is_empty() {
        for problem in problems {
            println!("ERROR {problem}");
        }
        return Err(CmdError::silent(1));
    }
    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    match where_ {
        Some(path) => println!("config ok ({})", path.display()),
        None => println!("config ok (defaults; no config file)"),
    }
    Ok(())
}

/// `config show`: the resolved values for the operator-facing keys.
fn show() -> Result<(), CmdError> {
    // Keys mirror cli.py exactly (lowercased constant names).
    let mut resolved = Map::new();
    resolved.insert("project".into(), Value::from(config::project()));
    resolved.insert("bucket".into(), Value::from(config::bucket()));
    resolved.insert("region".into(), Value::from(config::region()));
    resolved.insert(
        "regions".into(),
        Value::Array(config::regions().iter().map(|r| Value::from(r.as_str())).collect()),
    );
    resolved.insert(
        "wc_providers".into(),
        Value::Array(config::wc_providers().iter().map(|p| Value::from(p.as_str())).collect()),
    );
    resolved.insert("wc_storage_backend".into(), Value::from(config::wc_storage_backend()));
    resolved.insert("wc_local_storage_path".into(), Value::from(config::wc_local_storage_path()));
    resolved.insert("azure_subscription_id".into(), Value::from(config::azure_subscription_id()));
    resolved.insert("azure_resource_group".into(), Value::from(config::azure_resource_group()));
    resolved.insert(
        "azure_locations".into(),
        Value::Array(config::azure_locations().iter().map(|l| Value::from(l.as_str())).collect()),
    );
    resolved.insert("dashboard_bind".into(), Value::from(config::dashboard_bind()));
    resolved.insert("dashboard_port".into(), Value::from(config::dashboard_port()));

    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    let mut out = Map::new();
    out.insert(
        "file".into(),
        where_.map(|p| Value::from(p.display().to_string())).unwrap_or(Value::Null),
    );
    out.insert("resolved".into(), Value::Object(resolved));
    println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
    Ok(())
}
