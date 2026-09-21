//! The detached half of `jeden-session`: place the session on the host with
//! the most room for it and hand the work to the queue, so the agent keeps
//! running after the terminal, the editor or the laptop that asked for it is
//! gone.
//!
//! Nothing here supervises a process. The queue already owns the durable
//! part — one claim per job, per-job heartbeats, the reaper that requeues a
//! worker that died, retained output, and `stado cancel` — so a detached
//! session is an ordinary queue job pinned to the host this placement chose
//! and sized from the kind's declared reservation.

use serde_json::json;

use super::{
    admission_refusal, candidates, checkout_path, ledger_root, live_capacity, probe_ready,
    service_endpoint, shell_quote, validate_component, workspace_expression, MANAGED_JEDEN,
};
use crate::cli::workload::catalog::WorkloadKind;
use crate::cli::CmdError;
use crate::queue::submit::{
    submit_batch, ResolvedHardwareProjection, SubmitOptions, CPU_MACHINE_TYPE,
};

/// Version of the record `stado workload start` returns. A reader that
/// understands this number knows every field below is present.
pub(super) const RECORD_SCHEMA_VERSION: u64 = 1;

/// The fleet service a session reaches its models through.
const MODEL_ROUTER_SERVICE: &str = "brama";

/// The two credentials a Jeden session needs, as vault coordinates rather
/// than values: the agent's own signing secret and the gateway bearer. The
/// worker's agent resolves them through its own grant and hands them to the
/// session's environment; nothing puts a value on a command line, and a
/// host whose grant does not expose them leaves the session queued for one
/// that does.
fn session_secrets() -> std::collections::BTreeMap<String, crate::models::JobSecretRef> {
    let mut secrets = std::collections::BTreeMap::new();
    secrets.insert(
        "WISENT_APP_AGENT_AUTH_SECRET".to_string(),
        crate::models::JobSecretRef {
            item: "agent:wisent-app".to_string(),
            field: "value".to_string(),
        },
    );
    secrets.insert(
        "BRAMA_TOKEN".to_string(),
        crate::models::JobSecretRef {
            item: "jeden-model-router".to_string(),
            field: "token".to_string(),
        },
    );
    secrets
}

/// Everything one detached start is asked for. The grants are explicit
/// because nobody is watching the session: a detached run that could write
/// files and run commands by default would be an approval the operator never
/// gave.
pub(crate) struct DetachedRequest<'a> {
    pub kind: &'a WorkloadKind,
    pub workspace: &'a str,
    pub target: Option<&'a str>,
    pub task: &'a str,
    pub model: Option<&'a str>,
    pub max_steps: u32,
    pub priority: i64,
    pub allow_write: bool,
    pub allow_command: bool,
    pub json: bool,
}

pub(crate) async fn start_detached(request: DetachedRequest<'_>) -> Result<(), CmdError> {
    let kind = request.kind.kind.as_str();
    request.kind.require_detachable()?;
    validate_component("workspace", request.workspace)?;
    if request.task.trim().is_empty() {
        return Err(CmdError::usage(format!(
            "{kind} requires --task TEXT for a detached session"
        )));
    }
    let reservation = request.kind.reservation()?;
    let mut hosts = candidates(request.target).await?;
    if hosts.is_empty() {
        return Err(CmdError::click(format!(
            "the registry declares no reachable local host for {kind}; add it to the canonical registry"
        )));
    }
    let checkout = checkout_path(request.workspace);
    let capacity = live_capacity().await;
    let mut refusals = Vec::new();
    for target in hosts.drain(..) {
        // A pinned session only ever runs where it was placed, so a host
        // whose agent says it is not admitting work would hold it queued
        // instead of running it.
        if let Some(refusal) = admission_refusal(&target, &capacity) {
            refusals.push(refusal);
            continue;
        }
        let readiness = match probe_ready(&target, &checkout, None).await {
            Ok(readiness) => readiness,
            Err(refusal) => {
                refusals.push(refusal);
                continue;
            }
        };
        let router = service_endpoint(MODEL_ROUTER_SERVICE, &target.name).await;
        let command = session_command(&checkout, &request, router.as_deref());
        let run_id = run_id(kind, request.workspace);
        let options = SubmitOptions {
            batch_id: batch_id(kind),
            run_id: run_id.clone(),
            priority: request.priority,
            pinned_host: crate::cli::submit::resolve_pinned_host(&target.name).await?,
            cpu_cores: reservation.cpu_cores,
            memory_gb: reservation.ram_gb.ceil() as i64,
            // A host that holds the operator grant file reads the agent's
            // signing secret and the gateway bearer itself, exactly as an
            // attached session does. A host without it — the Linux builder
            // has none — is handed the same two credentials by its own
            // agent, from the fleet's grant, as declared job secrets.
            secret_env: if readiness.reads_own_credentials {
                Default::default()
            } else {
                session_secrets()
            },
            // A session reaches its model over HTTP through Brama, so it
            // needs no accelerator. Saying so explicitly also keeps the
            // submit-time sizing regex off the task text: a task that
            // mentions a model by name would otherwise be sized onto a GPU
            // host and then wait for VRAM no session ever uses.
            resolved_hardware: Some(ResolvedHardwareProjection {
                gpu_mem_gb: 0,
                gpu_type: String::new(),
                machine_type: CPU_MACHINE_TYPE.to_string(),
            }),
            ..SubmitOptions::default()
        };
        let jobs = submit_batch(std::slice::from_ref(&command), &options)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{kind} could not be queued on {}: {error}",
                    target.name
                ))
            })?;
        let job = jobs.first().ok_or_else(|| {
            CmdError::click(format!(
                "{kind} submission on {} returned no job; nothing is running",
                target.name
            ))
        })?;
        let (_, ledger) = ledger_root(&target);
        let record = json!({
            "schema_version": RECORD_SCHEMA_VERSION,
            "kind": kind,
            "target": target.name,
            "workspace": request.workspace,
            "cwd": format!("~/{checkout}"),
            "ledger": ledger,
            "job_id": job.job_id,
            "batch_id": options.batch_id,
            "priority": request.priority,
            "run_id": run_id,
            "task": request.task,
            "model": request.model,
            "state": "queued",
            "grants": {"write": request.allow_write, "command": request.allow_command},
            "reservation": {
                "cpu_cores": reservation.cpu_cores,
                "ram_gb": reservation.ram_gb,
            },
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
                "holds {} core(s) and {} GiB; session ledger {ledger}",
                reservation.cpu_cores, reservation.ram_gb
            );
            println!(
                "follow it with `stado job watch {} --follow`, stop it with `stado cancel {}`",
                job.job_id, job.job_id
            );
        }
        return Ok(());
    }
    Err(CmdError::click(format!(
        "no Stado host can start {kind} in {}; {}",
        request.workspace,
        refusals.join("; ")
    )))
}

/// The batch every detached session of one kind carries, so `stado status`
/// and `stado workload sessions` can find them without a second store.
pub(super) fn batch_id(kind: &str) -> String {
    format!("workload-{kind}")
}

/// A caller-retained identity that also records which workspace the session
/// works in; `stado workload sessions` reads the workspace back out of it.
fn run_id(kind: &str, workspace: &str) -> String {
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%S%3f");
    format!("workload-{kind}-{workspace}-{now}-{}", std::process::id())
}

/// The workspace a run id was started in, or nothing when the id was not
/// written by this command.
pub(super) fn workspace_of(kind: &str, run_id: &str) -> Option<String> {
    let rest = run_id.strip_prefix(&format!("workload-{kind}-"))?;
    let mut parts = rest.rsplitn(3, '-');
    let _pid = parts.next()?;
    let _stamp = parts.next()?;
    parts.next().map(str::to_string)
}

/// The one command the worker runs: the managed Jeden, in the workspace
/// checkout on that host, with the grants the operator asked for and nothing
/// else. `--json` keeps the session's final answer machine-readable in the
/// job's retained output.
///
/// It preflights itself first, because the worker is not the operator's
/// shell: a Stado agent started by launchd has no access to `~/Documents`
/// until macOS is told to give it some, and without this check the session
/// died printing the harness's usage text, which says nothing about why.
/// The two sentences below are what the job's retained log carries instead.
fn session_command(checkout: &str, request: &DetachedRequest<'_>, router: Option<&str>) -> String {
    let workspace = workspace_expression(checkout);
    let router_env = router
        .map(|url| format!("BRAMA_URL={} ", shell_quote(url)))
        .unwrap_or_default();
    let mut command = format!(
        "if ! cd {workspace} 2>/dev/null; then printf 'the worker cannot enter %s on this host; grant the Stado agent access to that directory or start the session in a workspace it can read\\n' {workspace} >&2; exit 1; fi; \
         if [ ! -x \"$HOME\"/{MANAGED_JEDEN} ]; then printf 'the managed Jeden is missing or not executable at %s on this host; install it with `stado product install jeden`\\n' \"$HOME\"/{MANAGED_JEDEN} >&2; exit 1; fi; \
         {router_env}PATH=\"$HOME/.stado/bin:$PATH\" exec \"$HOME\"/{MANAGED_JEDEN} run {} --json --max-steps {}",
        shell_quote(request.task),
        request.max_steps
    );
    if let Some(model) = request.model {
        command.push_str(&format!(" --model {}", shell_quote(model)));
    }
    if request.allow_write {
        command.push_str(" --allow-write");
    }
    if request.allow_command {
        command.push_str(" --allow-command");
    }
    command
}
