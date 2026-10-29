//! The two workload verbs that produce no receipt: attaching to an
//! interactive workload, and starting a detachable session.
//!
//! Split out of `workload/commands.rs`, which had grown past the module line
//! cap; the verbs that place and report a run stay there.

use crate::cli::CmdError;

use crate::cli::workload::catalog::{workload, DECLARATION_PATH};
use crate::cli::workload::plan::place;
use crate::cli::workload::runners::{
    connect_jeden, current_workspace, start_detached, start_omp, DetachedRequest, OmpRequest,
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
            // No --workspace names the workspace the caller stands in.
            let workspace = match workspace {
                Some(named) => named.to_string(),
                None => current_workspace(),
            };
            connect_jeden(declaration, &workspace, target.as_deref(), resume).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no stream attachment; add it to {DECLARATION_PATH}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)),
    }
}

pub(super) struct StartArguments<'a> {
    pub kind: &'a str,
    pub target: Option<&'a str>,
    pub workspace: Option<&'a str>,
    pub task: Option<&'a str>,
    pub model: Option<&'a str>,
    pub max_steps: Option<u32>,
    pub max_time: Option<&'a str>,
    pub context: &'a [std::path::PathBuf],
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
    let workspace = match arguments.workspace {
        Some(named) => named.to_string(),
        None => current_workspace(),
    };
    let Some(task) = arguments.task else {
        return Err(CmdError::usage(format!(
            "{kind} requires --task TEXT for a detached session"
        )));
    };
    if kind == "omp-session" {
        if arguments.max_steps.is_some() {
            return Err(CmdError::usage(format!(
                "{kind} is bounded by --max-time; OMP counts no steps, so drop --max-steps"
            )));
        }
        let Some(max_time) = arguments.max_time else {
            return Err(CmdError::usage(format!(
                "{kind} requires --max-time DURATION (seconds, or 30m, 8h): nobody watches a \
                 detached session, so its bound is the caller's to state"
            )));
        };
        return start_omp(OmpRequest {
            kind: declaration,
            workspace: &workspace,
            target: target.as_deref(),
            task,
            model: arguments.model,
            max_time,
            context: arguments.context,
            priority: arguments.priority,
            allow_write: arguments.allow_write,
            allow_command: arguments.allow_command,
            json: arguments.json,
        })
        .await;
    }
    if arguments.max_time.is_some() {
        return Err(CmdError::usage(format!(
            "{kind} is bounded by --max-steps, not --max-time"
        )));
    }
    if !arguments.context.is_empty() {
        return Err(CmdError::usage(format!(
            "{kind} reads its instructions from its workspace; --context is omp-session's"
        )));
    }
    let Some(max_steps) = arguments.max_steps else {
        return Err(CmdError::usage(format!(
            "{kind} requires --max-steps N: nobody watches a detached session, so its bound is \
             the caller's to state"
        )));
    };
    start_detached(DetachedRequest {
        kind: declaration,
        workspace: &workspace,
        target: target.as_deref(),
        task,
        model: arguments.model,
        max_steps,
        priority: arguments.priority,
        allow_write: arguments.allow_write,
        allow_command: arguments.allow_command,
        json: arguments.json,
    })
    .await
}
