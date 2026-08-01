use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference_process, production_runner};

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

fn succeeded(report: &Value, expected: &str) -> bool {
    report.get("status").and_then(Value::as_str) == Some(expected)
}

pub async fn blockers(host: &str, json_output: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let report = inference_process::blockers(&target, &production_runner())
        .await
        .map_err(click)?;
    if !succeeded(&report, "inventoried") {
        return Err(CmdError::click(format!(
            "GPU blocker inspection failed: {report}"
        )));
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let processes = report
        .get("processes")
        .and_then(Value::as_array)
        .expect("blocker report carries a process array");
    if processes.is_empty() {
        println!("no GPU compute processes on {host}");
        return Ok(());
    }
    println!("IDENTITY\tVRAM_MIB\tOWNER\tCOMMAND");
    for process in processes {
        println!(
            "{}\t{}\t{}\t{}",
            process
                .get("identity")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            process
                .get("used_mib")
                .and_then(Value::as_u64)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            process
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            process
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        );
    }
    Ok(())
}

pub async fn release(
    host: &str,
    identity: &str,
    force: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let report = inference_process::release(&target, identity, force, &production_runner())
        .await
        .map_err(click)?;
    if !succeeded(&report, "released") {
        return Err(CmdError::click(format!(
            "GPU process release failed: {report}"
        )));
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("released GPU process {identity} on {host}");
    }
    Ok(())
}
