//! The `stado workload` verbs and the dispatch that places each one.

use clap::Subcommand;
use serde_json::Value;

use crate::cli::CmdError;

use super::catalog::{list, workload, DECLARATION_PATH};
use super::plan::{
    boolean, dynamic_allowance, place, read_plan, required_plan, required_plan_path, required_text,
    string_array,
};
use super::runners::{
    list_sessions, mobile_runtime, refresh_weles_api_runtime, run_gui_automation,
    run_weles_browser_task, run_weles_capture, run_weles_diagnostics, run_weles_image_inspect,
    set_weles_recordings_dir, weles_activity, weles_browser_runtime,
};

mod interactive;
mod status;

use interactive::{attach, default_max_steps, start, StartArguments};
use status::status;

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
        /// Workspace name on the selected host; __home__ selects its home.
        #[arg(long)]
        workspace: Option<String>,
        /// Select only a host that owns this durable session ledger.
        #[arg(long)]
        resume: Option<String>,
    },
    /// Start a detachable workload that keeps running without this process.
    Start {
        kind: String,
        /// Pin placement to one registry target.
        #[arg(long)]
        target: Option<String>,
        /// Workspace name on the selected host; __home__ selects its home.
        #[arg(long)]
        workspace: Option<String>,
        /// The work the session is started for.
        #[arg(long)]
        task: Option<String>,
        /// Model route the session runs on; the harness default otherwise.
        #[arg(long)]
        model: Option<String>,
        /// Upper bound on the session's steps.
        #[arg(long, default_value_t = default_max_steps())]
        max_steps: u32,
        /// Let the session write files in its workspace.
        #[arg(long)]
        allow_write: bool,
        /// Let the session run commands in its workspace.
        #[arg(long)]
        allow_command: bool,
        /// Queue priority; higher is claimed first.
        #[arg(long, default_value_t = crate::primitives::constants::DETACHED_SESSION_JOB_PRIORITY)]
        priority: i64,
        /// Emit the placement record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List the detached sessions the fleet is running.
    Sessions {
        /// Emit the sessions as JSON.
        #[arg(long)]
        json: bool,
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
        WorkloadCommands::Attach {
            kind,
            target,
            workspace,
            resume,
        } => {
            attach(
                &kind,
                target.as_deref(),
                workspace.as_deref(),
                resume.as_deref(),
            )
            .await
        }
        WorkloadCommands::Start {
            kind,
            target,
            workspace,
            task,
            model,
            max_steps,
            allow_write,
            allow_command,
            priority,
            json,
        } => {
            start(StartArguments {
                kind: &kind,
                target: target.as_deref(),
                workspace: workspace.as_deref(),
                task: task.as_deref(),
                model: model.as_deref(),
                max_steps,
                allow_write,
                allow_command,
                priority,
                json,
            })
            .await
        }
        WorkloadCommands::Sessions { json } => list_sessions(json).await,
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
    // The host is chosen; take the kind's declared hold on it for the whole
    // run, so the host publishes itself net of this work from its next tick.
    // A maintenance kind takes none: holding capacity on a host in order to
    // repair it is how a full disk becomes permanent.
    if declaration.maintenance {
        return run_kind(kind, target, plan.as_ref(), document, json_output).await;
    }
    let holder = format!(
        "{} {kind} pid {}",
        crate::fleet_needs::this_requester(),
        std::process::id()
    );
    let held =
        match crate::cli::capacity::reserve_for_workload(declaration, &resolved, holder).await? {
            Ok(held) => held,
            Err(refusal) => return Err(refusal.into_error()),
        };
    let outcome = run_kind(kind, target, plan.as_ref(), document, json_output).await;
    if let Err(error) = held.release().await {
        eprintln!("the workload's reservation could not be released: {error}");
    }
    outcome
}

async fn run_kind(
    kind: &str,
    target: &str,
    plan: Option<&(Value, &str)>,
    document: Option<&Value>,
    json_output: bool,
) -> Result<(), CmdError> {
    match kind {
        "weles-capture" => {
            run_weles_capture(target, required_plan_path(plan, kind)?, json_output).await
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

