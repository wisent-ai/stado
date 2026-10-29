//! The `omp-session` workload: one OMP coding-agent session placed on the
//! host with the most room for it, detached as a durable queue job.
//!
//! It shares the session placement of `jeden-session` — the candidates, the
//! hosts' own capacity publications, the batch the fleet lists sessions by —
//! and differs in what runs: the harness the kind declares, at the managed
//! path the fleet delivers it to (`stado workload harness`), signed into the
//! pool's accounts by Brama's hand-over on that host. The workspace is the
//! host's one checkout of the repository, cloned from this machine's origin
//! when the host has none. The session writes its result lines below the
//! job's output directory, where `stado workload results` reads them.

mod command;
mod context;
mod harness;
mod results;

pub(crate) use harness::{install as install_harness, status as harness_status};
pub(crate) use results::collect as collect_results;

use serde_json::json;

use super::capacity::{admission_refusal, live_capacity};
use super::detached::{batch_id, run_id, RECORD_SCHEMA_VERSION};
use super::{candidates, checkout_path, validate_component, CHECKOUT_ROOT, HOME_WORKSPACE};
use crate::cli::workload::catalog::WorkloadKind;
use crate::cli::CmdError;
use crate::queue::submit::{
    submit_batch, ResolvedHardwareProjection, SubmitOptions, CPU_MACHINE_TYPE,
};

/// Everything one `omp-session` start is asked for.
pub(crate) struct OmpRequest<'a> {
    pub kind: &'a WorkloadKind,
    pub workspace: &'a str,
    pub target: Option<&'a str>,
    pub task: &'a str,
    pub model: Option<&'a str>,
    pub max_time: &'a str,
    pub context: &'a [std::path::PathBuf],
    pub priority: i64,
    pub allow_write: bool,
    pub allow_command: bool,
    pub json: bool,
}

/// The origin this machine's checkout of `workspace` fetches from: the
/// host's own checkout is cloned from the same place.
async fn origin(workspace: &str) -> Result<String, CmdError> {
    let checkout = super::expand_home(&format!("{CHECKOUT_ROOT}/{workspace}"))?;
    let output = crate::wait::output_async(
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["remote", "get-url", "origin"]),
    )
    .await?;
    if !output.status.success() {
        return Err(CmdError::usage(format!(
            "{} has no origin this machine can read ({}); a session works in a canonical \
             checkout, so name a workspace that is one",
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The job secrets the declaration hands a session, as the role and field
/// the host's agent reads each from. A reference that is not `role#field` is
/// refused before anything is queued.
fn job_secrets(
    kind: &str,
    harness: &crate::cli::workload::catalog::WorkloadHarness,
) -> Result<std::collections::BTreeMap<String, crate::models::JobSecretRef>, CmdError> {
    harness
        .secrets
        .iter()
        .map(|(name, reference)| {
            let (role, field) = reference.split_once('#').ok_or_else(|| {
                CmdError::declaration(format!(
                    "workload kind '{kind}' declares secret {name} as {reference:?}, not \
                     role#field; fix it in {}",
                    crate::cli::workload::DECLARATION_PATH
                ))
            })?;
            Ok((
                name.clone(),
                crate::models::JobSecretRef {
                    role: role.to_string(),
                    field: field.to_string(),
                },
            ))
        })
        .collect()
}

pub(crate) async fn start_omp(request: OmpRequest<'_>) -> Result<(), CmdError> {
    let kind = request.kind.kind.as_str();
    request.kind.require_detachable()?;
    validate_component("workspace", request.workspace)?;
    if request.workspace == HOME_WORKSPACE {
        return Err(CmdError::usage(format!(
            "{kind} works in one repository's checkout; name it with --workspace"
        )));
    }
    if request.task.trim().is_empty() {
        return Err(CmdError::usage(format!(
            "{kind} requires --task TEXT for a detached session"
        )));
    }
    let harness = request.kind.harness.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "workload kind '{kind}' declares no harness; add it to {}",
            crate::cli::workload::DECLARATION_PATH
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let reservation = request.kind.reservation()?;
    let secrets = job_secrets(kind, harness)?;
    let origin = origin(request.workspace).await?;
    let checkout = checkout_path(request.workspace);
    let capacity = live_capacity().await?;
    let mut refusals = Vec::new();
    for target in candidates(request.target).await? {
        if let Some(refusal) = admission_refusal(&target, &capacity) {
            refusals.push(refusal);
            continue;
        }
        let command = command::session_command(&command::Session {
            checkout: &checkout,
            origin: &origin,
            harness,
            task: request.task,
            model: request.model,
            max_time: request.max_time,
            allow_write: request.allow_write,
            allow_command: request.allow_command,
        });
        let run_id = run_id(kind, request.workspace);
        let secret_env = secrets.clone();
        let resolved_input_artifacts = context::inputs(&run_id, request.context).await?;
        let options = SubmitOptions {
            batch_id: batch_id(kind),
            run_id: run_id.clone(),
            priority: request.priority,
            pinned_host: crate::cli::submit::resolve_pinned_host(&target.name).await?,
            cpu_cores: reservation.cpu_cores,
            memory_gb: reservation.ram_gb.ceil() as i64,
            secret_env,
            resolved_input_artifacts,
            // The model is reached over HTTP; a task that names one must not
            // be sized onto an accelerator by the submit-time reading.
            resolved_hardware: Some(ResolvedHardwareProjection {
                gpu_mem_gb: Default::default(),
                gpu_type: String::new(),
                machine_type: CPU_MACHINE_TYPE.to_string(),
            }),
            ..SubmitOptions::default()
        };
        let jobs = submit_batch(std::slice::from_ref(&command), &options)
            .await
            .map_err(|error| {
                let mut wrapped = CmdError::click(format!(
                    "{kind} could not be queued on {}: {error}",
                    target.name
                ));
                wrapped.failure = CmdError::from(error).failure;
                wrapped
            })?;
        let job = jobs.first().ok_or_else(|| {
            CmdError::click(format!(
                "{kind} submission on {} returned no job; nothing is running",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let record = json!({
            "schema_version": RECORD_SCHEMA_VERSION,
            "kind": kind,
            "target": target.name,
            "workspace": request.workspace,
            "cwd": format!("~/{checkout}"),
            "origin": origin,
            "job_id": job.job_id,
            "batch_id": options.batch_id,
            "priority": request.priority,
            "run_id": run_id,
            "task": request.task,
            "model": request.model,
            "max_time": request.max_time,
            "state": "queued",
            "grants": {"write": request.allow_write, "command": request.allow_command},
            "reservation": {"cpu_cores": reservation.cpu_cores, "ram_gb": reservation.ram_gb},
            "command": command,
        });
        if request.json {
            crate::cli::workload::plan::print_json(&record);
        } else {
            println!(
                "{kind} queued on {} as job {} in ~/{checkout}",
                target.name, job.job_id
            );
            println!(
                "follow it with `stado job watch {} --follow-seconds <SECONDS>`, collect its \
                 results with `stado workload results {} --into <DIR>`, stop it with `stado \
                 cancel {}`",
                job.job_id, job.job_id, job.job_id
            );
        }
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "no Stado host can start {kind} in {}; {}",
        request.workspace,
        refusals.join("; ")
    )))
}
