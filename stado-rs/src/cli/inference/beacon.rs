use std::process::Command;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::inference::reservation;

fn unit_state(name: &str) -> String {
    let unit = format!("stado-inference-{name}.service");
    let output =
        crate::wait::output(Command::new("systemctl").args(["--user", "is-active", &unit]));
    match output {
        Ok(output) if output.status.success() => "active".to_string(),
        Ok(output) => {
            let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if state.is_empty() {
                "inactive".to_string()
            } else {
                state
            }
        }
        Err(error) => format!("unknown: {error}"),
    }
}

/// The GPU memory nvidia-smi reports in use, or why it could not be read.
fn gpu_memory_used() -> Result<String, String> {
    let output = crate::wait::output(
        Command::new("nvidia-smi")
            .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]),
    )
    .map_err(|error| format!("nvidia-smi could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "nvidia-smi ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    match String::from_utf8_lossy(&output.stdout).lines().next() {
        Some(line) => Ok(line.trim().to_string()),
        None => Err("nvidia-smi printed no memory reading".to_string()),
    }
}

pub fn local() -> Result<(), CmdError> {
    let Some(active) = reservation::active().map_err(|error| {
        CmdError::refused(format!("the inference reservation cannot be read: {error}"))
    })?
    else {
        println!("{{}}");
        return Ok(());
    };
    let state = unit_state(&active.deployment);
    let mut entry = Map::new();
    entry.insert("state".to_string(), Value::String(state));
    entry.insert("gpu_mode".to_string(), json!(active.gpu_mode));
    // A reading that could not be taken is null with its cause beside it, so
    // a host without the tool is not read as a GPU with nothing in use.
    match gpu_memory_used() {
        Ok(used) => {
            entry.insert("gpu_memory_used_mb".to_string(), Value::String(used));
        }
        Err(cause) => {
            entry.insert("gpu_memory_used_mb".to_string(), Value::Null);
            entry.insert("gpu_memory_error".to_string(), Value::String(cause));
        }
    }
    entry.insert("engine".to_string(), json!(active.engine));
    entry.insert("model".to_string(), json!(active.model));
    entry.insert("revision".to_string(), json!(active.revision));
    entry.insert("endpoint_host".to_string(), json!(active.endpoint_host));
    entry.insert("port".to_string(), json!(active.port));
    let mut body = Map::new();
    body.insert(active.deployment, Value::Object(entry));
    println!("{}", serde_json::to_string(&Value::Object(body))?);
    Ok(())
}
