//! The attached machine's own Stado configuration: made to exist, and read
//! for the device-local store it keeps.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// The machine's configuration document (`config show --json`): its file,
/// `null` when it has none, and every resolved value.
async fn shown(target: &ComputeTarget, runner: &Runner) -> Result<Value, CmdError> {
    let shown = crate::cli::host::remote_config_output(
        target,
        crate::cli::host::RemoteConfigAction::Show,
        runner,
    )
    .await?;
    serde_json::from_str(&shown).map_err(|error| {
        CmdError::click(format!(
            "{} answered its configuration read with something that is not JSON ({error})",
            target.name
        ))
        .stating(FailureCode::InfraDown)
    })
}

/// Give a machine that runs on defaults alone a configuration file, with
/// its own `config init`, so the declarations attaching writes have a file
/// to land in: a standalone Stado refuses every `config set` until one
/// exists. A machine that has one keeps it as it is.
pub(super) async fn ensure_file(
    target: &ComputeTarget,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    if !shown(target, runner).await?["file"].is_null() {
        return Ok(());
    }
    let home = crate::deploy::host_channel::remote_home(target, runner)
        .await
        .map_err(CmdError::from)?;
    let program = format!("{home}/.stado/bin/stado");
    let output = crate::deploy::host_channel::run_program(
        target,
        &[program.as_str(), "config", "init"],
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{}: it has no configuration file and `stado config init` there failed: {}",
            target.name,
            crate::deploy::host_channel::whole_failure(&output)
        ))
        .stating(FailureCode::InfraDown));
    }
    echo(&format!(
        "[attach] {}: it had no configuration file; `stado config init` wrote {}",
        target.name,
        output.stdout.trim()
    ));
    Ok(())
}

/// The device-local store path the machine's configuration names now.
pub(super) async fn local_store(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, CmdError> {
    shown(target, runner).await?["resolved"]["wc_local_storage_path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: its configuration names no local storage path to keep",
                target.name
            ))
            .stating(FailureCode::InfraDown)
        })
}
