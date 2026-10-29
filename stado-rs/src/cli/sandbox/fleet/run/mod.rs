//! Running one fleet sandbox operation: on the host the caller named, or
//! the one Stado chose or found for it ([`placement`]), and reading the
//! receipt its program printed.

mod placement;
mod room;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::host_channel;
pub(super) use placement::Demand;

/// One operation as `prepare` built it: the host program and what decides
/// where it runs. `demand` is set for `create` only, `fork_to` for `fork`.
pub(super) struct Request {
    pub host: Option<String>,
    pub name: Option<String>,
    pub operation: &'static str,
    pub script: String,
    pub demand: Option<Demand>,
    pub fork_to: Option<String>,
}

pub(super) async fn run(request: Request) -> Result<(), CmdError> {
    let Request {
        host,
        name,
        operation,
        script,
        demand,
        fork_to,
    } = request;
    let placed = match (host, name.as_deref(), demand) {
        (Some(host), _, _) => placement::named(&host).await?,
        (None, None, _) => return placement::list_fleet().await,
        (None, Some(name), Some(demand)) => placement::place(name, &demand).await?,
        (None, Some(name), None) => placement::locate(name, fork_to.as_deref()).await?,
    };
    let target = &placed.target;
    let host = target.name.clone();
    if !target.release_platform.starts_with("linux-") {
        return Err(CmdError::refused(format!("{host}: fleet sandboxes require Linux with Docker, cgroup v2 and gVisor runsc; observed platform {}", target.release_platform)));
    }
    let runner = crate::deploy::production_runner();
    let output = host_channel::run_script(target, &script, &runner).await?;
    let mut result: Option<Value> = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("STADO_SANDBOX\t"))
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| {
            CmdError::refused(format!(
                "{host}: {operation} returned an unreadable sandbox receipt: {error}"
            ))
        })?;
    if operation == "list" && output.succeeded() {
        let sandboxes = output
            .stdout
            .lines()
            .filter(|line| !line.starts_with("STADO_SANDBOX\t") && !line.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CmdError::refused(format!(
                    "{host}: Docker returned an unreadable sandbox listing: {error}"
                ))
            })?;
        result = Some(json!({"sandboxes":sandboxes}));
    }
    let kernel_limits = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("STADO_SANDBOX_LIMITS\t"))
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|error| {
            CmdError::refused(format!(
                "{host}: {operation} returned unreadable kernel resource observations: {error}"
            ))
        })?;
    let report = json!({"schema":"stado.fleet-sandbox.v1", "host":host,
        "operation":operation, "ok":output.succeeded(), "exit_code":output.code,
        "stdout":output.stdout, "stderr":output.stderr, "result":result,
        "kernel_limits":kernel_limits, "placement":placed.placement});
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !output.succeeded() {
        return Err(CmdError::refused(format!(
            "{host}: sandbox {operation} failed with exit {}; stderr: {}; stdout: {}",
            output.code,
            output.stderr.trim(),
            output.stdout.trim()
        )));
    }
    if result.is_none() {
        return Err(CmdError::refused(format!(
            "{host}: sandbox {operation} exited without an observed-state receipt"
        )));
    }
    Ok(())
}
