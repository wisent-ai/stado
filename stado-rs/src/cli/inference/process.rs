use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::{inference::process, production_runner};

fn succeeded(report: &Value, expected: &str) -> bool {
    report.get("status").and_then(Value::as_str) == Some(expected)
}

pub async fn blockers(host: &str, json_output: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let report = process::blockers(&target, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !succeeded(&report, "inventoried") {
        return Err(
            CmdError::click(format!("GPU blocker inspection failed: {report}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
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
    // The identity is what `release` takes, so a row without one is refused
    // rather than printed as "unknown"; the other columns are display only
    // and say they were not reported.
    println!("IDENTITY\tVRAM_MIB\tOWNER\tCOMMAND");
    for process in processes {
        let Some(identity) = process.get("identity").and_then(Value::as_str) else {
            return Err(CmdError::click(format!(
                "GPU blocker report from {host} lists a process without an identity, so it cannot be released: {process}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        };
        let vram = match process.get("used_mib").and_then(Value::as_u64) {
            Some(value) => value.to_string(),
            None => NOT_REPORTED.to_string(),
        };
        let text = |field: &str| match process.get(field).and_then(Value::as_str) {
            Some(value) => value.to_string(),
            None => NOT_REPORTED.to_string(),
        };
        println!("{identity}\t{vram}\t{}\t{}", text("owner"), text("command"));
    }
    Ok(())
}

const NOT_REPORTED: &str = "not reported";

pub async fn release(
    host: &str,
    identity: &str,
    force: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let report = process::release(&target, identity, force, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !succeeded(&report, "released") {
        return Err(
            CmdError::click(format!("GPU process release failed: {report}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("released GPU process {identity} on {host}");
    }
    Ok(())
}
