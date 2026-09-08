//! The `stado workload` verbs and the dispatch that places each one.

use clap::Subcommand;
use serde_json::Value;

use crate::cli::CmdError;

use super::catalog::{catalog, list, workload, DECLARATION_PATH};
use super::plan::{
    boolean, dynamic_allowance, place, read_plan, required_plan, required_plan_path, required_text,
    string_array,
};
use super::runners::{
    connect_jeden, current_workspace, gui_automation_status, mobile_runtime, recordings_status,
    refresh_weles_api_runtime, run_gui_automation, run_weles_browser_task, run_weles_capture,
    run_weles_diagnostics, run_weles_image_inspect, set_weles_recordings_dir, weles_activity,
    weles_browser_runtime, weles_capture_status, weles_run_diagnostics,
};

#[derive(Subcommand)]
pub enum WorkloadCommands {
    /// List every workload kind compiled into this Stado build.
    List {
        /// Emit the declaration as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Place and run one declared non-interactive workload.
    Run {
        kind: String,
        /// Pin placement to one registry target.
        #[arg(long)]
        target: Option<String>,
        /// JSON plan whose schema is declared by the workload kind.
        #[arg(long)]
        plan: Option<String>,
        /// Emit the workload's report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read the latest report for a workload kind or receipt identifier.
    Status {
        kind_or_id: String,
        /// Read the report from one registry target.
        #[arg(long)]
        target: Option<String>,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Attach this process's stdin and stdout to an interactive workload.
    Attach {
        kind: String,
        /// Reattach on one registry target rather than placing afresh.
        #[arg(long)]
        target: Option<String>,
    },
}

pub async fn dispatch(command: WorkloadCommands) -> Result<(), CmdError> {
    match command {
        WorkloadCommands::List { json } => list(json),
        WorkloadCommands::Run {
            kind,
            target,
            plan,
            json,
        } => run(&kind, target.as_deref(), plan.as_deref(), json).await,
        WorkloadCommands::Status {
            kind_or_id,
            target,
            json,
        } => status(&kind_or_id, target.as_deref(), json).await,
        WorkloadCommands::Attach { kind, target } => attach(&kind, target.as_deref()).await,
    }
}

async fn run(
    kind: &str,
    requested_target: Option<&str>,
    plan_path: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let declaration = workload(kind)?;
    if declaration.interactive {
        if json_output {
            return Err(CmdError::usage(format!(
                "{kind} is interactive and cannot produce JSON; use `stado workload attach {kind}`"
            )));
        }
        return Err(CmdError::usage(format!(
            "{kind} is interactive; use `stado workload attach {kind}`"
        )));
    }
    let plan = read_plan(declaration, plan_path)?;
    let document = plan.as_ref().map(|(document, _)| document);
    let allowance = dynamic_allowance(declaration, document);
    let plan_target = if kind == "weles-capture" {
        document
            .and_then(|value| value.get("target"))
            .and_then(Value::as_str)
    } else {
        None
    };
    let resolved = place(declaration, requested_target.or(plan_target), allowance).await?;
    let target = resolved.name.as_str();

    match kind {
        "weles-capture" => {
            run_weles_capture(
                target,
                required_plan_path(plan.as_ref(), kind)?,
                json_output,
            )
            .await
        }
        "weles-browser-task" => {
            run_weles_browser_task(target, required_plan(document, kind)?, json_output).await
        }
        "weles-diagnostics" => {
            run_weles_diagnostics(target, required_plan(document, kind)?, json_output).await
        }
        "weles-image-inspect" => {
            run_weles_image_inspect(target, required_text(document, "url")?, json_output).await
        }
        "weles-activity" => weles_activity(target, json_output).await,
        "weles-recordings" => {
            set_weles_recordings_dir(target, required_text(document, "path")?, json_output).await
        }
        "weles-api-runtime" => {
            refresh_weles_api_runtime(target, required_text(document, "revision")?, json_output)
                .await
        }
        "weles-browser-runtime" => {
            let document = required_plan(document, kind)?;
            let components = string_array(document, "components")?;
            weles_browser_runtime(
                target,
                &components,
                boolean(document, "repair", false),
                json_output,
            )
            .await
        }
        "gui-automation" => {
            run_gui_automation(target, required_plan(document, kind)?, json_output).await
        }
        "mobile-runtime" => {
            let document = required_plan(document, kind)?;
            mobile_runtime(target, boolean(document, "repair", false), json_output).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} has no runner; add it to {DECLARATION_PATH}"
        ))),
    }
}

async fn status(
    selector: &str,
    requested_target: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let (kind, receipt) = if let Some((kind, id)) = selector.split_once(':') {
        (kind, Some(id))
    } else if catalog()?
        .workloads
        .iter()
        .any(|entry| entry.kind == selector)
    {
        (selector, None)
    } else {
        ("weles-capture", Some(selector))
    };
    let declaration = workload(kind)?;
    if declaration.interactive {
        return Err(CmdError::usage(format!(
            "{kind} is interactive and has no receipt report; use `stado workload attach {kind}`"
        )));
    }
    let resolved = place(
        declaration,
        requested_target,
        dynamic_allowance(declaration, None),
    )
    .await?;
    let target = resolved.name.as_str();
    match (kind, receipt) {
        ("weles-capture", Some(batch)) => weles_capture_status(target, batch, json_output).await,
        ("weles-capture", None) => Err(CmdError::usage(
            "weles-capture status needs its receipt id: `stado workload status weles-capture:<batch>`",
        )),
        ("weles-diagnostics", Some(run_id)) => weles_run_diagnostics(target, run_id, None, json_output).await,
        ("weles-diagnostics", None) => Err(CmdError::usage(
            "weles-diagnostics status needs its receipt id: `stado workload status weles-diagnostics:<run-id>`",
        )),
        ("weles-browser-runtime", _) => weles_browser_runtime(target, &[], false, json_output).await,
        ("mobile-runtime", _) => mobile_runtime(target, false, json_output).await,
        ("gui-automation", _) => gui_automation_status(target, json_output).await,
        ("weles-recordings", _) => recordings_status(&resolved, json_output),
        ("weles-activity" | "weles-browser-task" | "weles-image-inspect" | "weles-api-runtime", _) => {
            weles_activity(target, json_output).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no status report; add it to {DECLARATION_PATH}"
        ))),
    }
}

async fn attach(kind: &str, requested_target: Option<&str>) -> Result<(), CmdError> {
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
            let workspace = current_workspace();
            connect_jeden(&workspace, target.as_deref(), None).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no stream attachment; add it to {DECLARATION_PATH}"
        ))),
    }
}
