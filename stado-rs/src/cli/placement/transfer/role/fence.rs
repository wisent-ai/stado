//! The store fence a role move arms and lifts on a host, through that
//! host's own `stado storage fence`.

use serde_json::Value;

use super::super::run_host_script;
use crate::cli::CmdError;
use crate::deploy::{shlex_quote, Runner};
use crate::targets::ComputeTarget;

/// `stado storage fence <verb>` on `host` for `transaction`; the `state`
/// its receipt answers.
pub(super) async fn fence(
    host: &ComputeTarget,
    verb: &str,
    transaction: &str,
    reason: &str,
    runner: &Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -eu\n\"$HOME/.stado/bin/stado\" storage fence {verb} --transaction {} --reason {} \
         --root \"$HOME\"/{} --json\n",
        shlex_quote(transaction),
        shlex_quote(reason),
        shlex_quote(crate::config_file::LOCAL_STORAGE_ROOT)
    );
    let receipt = answered(host, &script, &format!("storage fence {verb}"), runner).await?;
    receipt["state"].as_str().map(str::to_string).ok_or_else(|| {
        CmdError::click(format!(
            "{}: stado storage fence {verb} answered no state: {receipt}",
            host.name
        ))
    })
}

/// The intent of the write fence on `host`'s store (`null` when the store is
/// not fenced), as that host's own `stado storage fence status` reads it: a
/// host a role move left keeps its copy fenced, which `placement holds` says.
pub(in crate::cli::placement) async fn store_fence(host: &ComputeTarget, runner: &Runner) -> Result<Value, CmdError> {
    let script = format!(
        "set -eu\n\"$HOME/.stado/bin/stado\" storage fence status --root \"$HOME\"/{} --json\n",
        shlex_quote(crate::config_file::LOCAL_STORAGE_ROOT)
    );
    Ok(answered(host, &script, "storage fence status", runner).await?["intent"].clone())
}

/// The transaction of the fence an earlier role move left on `host`'s store
/// (one `stado storage fence` armed), which a move landing there lifts once
/// it commits; `None` when the store is not fenced. A fence another kind of
/// transaction holds (a storage-root reconcile) is refused: the move would
/// land on a store that refuses every write.
pub(super) async fn left_fence(host: &ComputeTarget, runner: &Runner) -> Result<Option<String>, CmdError> {
    // The store a role carries, named outright: a host whose own storage is
    // the object API declares no local storage path of its own.
    let state = store_fence(host, runner).await?;
    let intent = &state;
    if intent.is_null() {
        return Ok(None);
    }
    let transaction = intent["transaction"].as_str().ok_or_else(|| {
        CmdError::click(format!("{}: its store's fence names no transaction: {intent}", host.name))
    })?;
    if intent["owner"].as_str() != Some(crate::cli::storage::FENCE_OWNER) {
        return Err(CmdError::refused(format!(
            "{}: its store is fenced by transaction {transaction}, which no role move armed, so a \
             store moved there would refuse every write; end that transaction first",
            host.name
        )));
    }
    Ok(Some(transaction.to_string()))
}

/// Run `script` on `host` and read the JSON receipt it prints.
async fn answered(
    host: &ComputeTarget,
    script: &str,
    operation: &str,
    runner: &Runner,
) -> Result<Value, CmdError> {
    let output = run_host_script(host, script, runner, operation).await?;
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: stado {operation} printed no receipt ({error}): {}",
            host.name,
            output.stdout.trim()
        ))
    })
}
