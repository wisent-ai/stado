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

/// Give a machine whose fleet configuration file is absent one, with its
/// own `config init` run as every fleet command reads and writes the host's
/// configuration, so the declarations attaching writes have a file to land
/// in: `config set` refuses until one exists. A standalone Stado's own
/// file elsewhere in its home is left as it is. A machine that has the file
/// keeps it as it is.
pub(super) async fn ensure_file(
    target: &ComputeTarget,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    if !shown(target, runner).await?["file"].is_null() {
        return Ok(());
    }
    let shown = crate::cli::host::remote_config_output(
        target,
        crate::cli::host::RemoteConfigAction::Init,
        runner,
    )
    .await?;
    let document: Value = serde_json::from_str(&shown).unwrap_or(Value::Null);
    echo(&format!(
        "[attach] {}: it had no fleet configuration file; `stado config init` wrote {}",
        target.name,
        document["file"].as_str().unwrap_or("one")
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
