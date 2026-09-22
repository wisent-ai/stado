//! The two workload verbs that produce no receipt: attaching to an
//! interactive workload, and starting a detachable session.
//!
//! Split out of `workload/commands.rs`, which had grown past the module line
//! cap; the verbs that place and report a run stay there.

use crate::cli::CmdError;

use crate::cli::workload::catalog::{workload, DECLARATION_PATH};
use crate::cli::workload::plan::place;
use crate::cli::workload::runners::{
    connect_jeden, current_workspace, start_detached, DetachedRequest,
};

pub(super) async fn attach(
    kind: &str,
    requested_target: Option<&str>,
    workspace: Option<&str>,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    let declaration = workload(kind)?;
    if !declaration.interactive {
        return Err(CmdError::usage(format!(
            "{kind} is not interactive; use `stado workload run {kind}`"
        )));
    }
    match kind {
        "jeden-session" => {
            let target = match requested_target {
                Some(name) => Some(place(declaration, Some(name), None).await?.name),
                None => None,
            };
            let workspace = workspace
                .map(str::to_string)
                .unwrap_or_else(current_workspace);
            connect_jeden(declaration, &workspace, target.as_deref(), resume).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no stream attachment; add it to {DECLARATION_PATH}"
        ))),
    }
}

/// How far a detached session may run before the harness stops it. High
/// enough for real work, bounded because nobody is watching it.
pub(super) fn default_max_steps() -> u32 {
    64
}

pub(super) struct StartArguments<'a> {
    pub kind: &'a str,
    pub target: Option<&'a str>,
    pub workspace: Option<&'a str>,
    pub task: Option<&'a str>,
    pub model: Option<&'a str>,
    pub max_steps: u32,
    pub priority: i64,
    pub allow_write: bool,
    pub allow_command: bool,
    pub json: bool,
}

pub(super) async fn start(arguments: StartArguments<'_>) -> Result<(), CmdError> {
    let kind = arguments.kind;
    let declaration = workload(kind)?;
    declaration.require_detachable()?;
    let target = match arguments.target {
        Some(name) => Some(place(declaration, Some(name), None).await?.name),
        None => None,
    };
    let workspace = arguments
        .workspace
        .map(str::to_string)
        .unwrap_or_else(current_workspace);
    let task = arguments.task.unwrap_or_default();
    start_detached(DetachedRequest {
        kind: declaration,
        workspace: &workspace,
        target: target.as_deref(),
        task,
        model: arguments.model,
        max_steps: arguments.max_steps,
        priority: arguments.priority,
        allow_write: arguments.allow_write,
        allow_command: arguments.allow_command,
        json: arguments.json,
    })
    .await
}
