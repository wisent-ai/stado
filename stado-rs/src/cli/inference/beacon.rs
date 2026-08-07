use std::process::Command;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::inference::reservation;

fn unit_state(name: &str) -> String {
    let unit = format!("stado-inference-{name}.service");
    let output = Command::new("systemctl")
        .args(["--user", "is-active", &unit])
        .output();
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

fn gpu_memory_used() -> Option<String> {
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    })
}

pub fn local() -> Result<(), CmdError> {
    let Some(active) = reservation::active() else {
        println!("{{}}");
        return Ok(());
    };
    let state = unit_state(&active.deployment);
    let mut entry = Map::new();
    entry.insert("state".to_string(), Value::String(state));
    entry.insert("gpu_mode".to_string(), Value::String(active.gpu_mode));
    entry.insert(
        "gpu_memory_used_mb".to_string(),
        gpu_memory_used().map_or(Value::Null, Value::String),
    );
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
