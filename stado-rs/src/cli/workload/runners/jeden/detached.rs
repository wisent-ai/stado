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
//!
//! The session is the fleet's, so it works where the host keeps the fleet's
//! work ([`super::probe::place`]): the host's one checkout of the repository
//! below its declared work root, cloned from this machine's origin when the
//! host has none, or, in the home workspace, its own job tree; its ledger is
//! below the work root and its Cargo target and temporary directories are in
//! the job tree. A host that declares no work root is refused.

use serde_json::json;

use super::probe::place::Place;
use super::sessions::{batch_id, run_id, RECORD_SCHEMA_VERSION};
use super::{
    admission_refusal, candidates, checkout_preamble, fleet_place, github_token_environment,
    job_tree_environment, ledger_environment, live_capacity, origin, probe_ready,
    service_endpoint, shell_quote, validate_component, HOME_WORKSPACE, MANAGED_JEDEN,
};
use crate::cli::workload::catalog::WorkloadKind;
use crate::cli::CmdError;
use crate::queue::submit::{
    submit_batch, ResolvedHardwareProjection, SubmitOptions, CPU_MACHINE_TYPE,
};

/// The fleet service a session reaches its models through.
const MODEL_ROUTER_SERVICE: &str = "brama";

/// The credentials a Jeden session needs, as roles rather than values: the
/// agent's own signing secret, the gateway bearer, and the GitHub token the
/// host's checkout is cloned and fetched with. The worker's agent selects
/// the items playing them through its own grant and hands their values to
/// the session's environment; nothing puts a value on a command line, and a
/// host whose grant exposes no item in a role leaves the session queued for
/// one that does.
const SESSION_SIGNING_ROLE: &str = "jeden-session-signing";
const SESSION_GATEWAY_ROLE: &str = "jeden-session-gateway";
const SESSION_GITHUB_ROLE: &str = "github";

fn session_secrets() -> std::collections::BTreeMap<String, crate::models::JobSecretRef> {
    let mut secrets = std::collections::BTreeMap::new();
    for (variable, role, field) in [
        ("WISENT_APP_AGENT_AUTH_SECRET", SESSION_SIGNING_ROLE, "value"),
        ("BRAMA_TOKEN", SESSION_GATEWAY_ROLE, "token"),
        ("GITHUB_TOKEN", SESSION_GITHUB_ROLE, "value"),
    ] {
        secrets.insert(
            variable.to_string(),
            crate::models::JobSecretRef {
                role: role.to_string(),
                field: field.to_string(),
            },
        );
    }
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
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    // The home workspace has no repository; every other one is cloned on
    // the host from the origin this machine's checkout fetches from.
    let origin = match request.workspace {
        HOME_WORKSPACE => None,
        workspace => Some(origin(workspace).await?),
    };
    let capacity = live_capacity().await?;
    let mut refusals = Vec::new();
    for target in hosts.drain(..) {
        let place = match fleet_place(&target, request.workspace) {
            Ok(place) => place,
            Err(refusal) => {
                refusals.push(refusal);
                continue;
            }
        };
        // A pinned session only ever runs where it was placed, so a host
        // whose agent says it is not admitting work would hold it queued
        // instead of running it.
        if let Some(refusal) = admission_refusal(&target, &capacity) {
            refusals.push(refusal);
            continue;
        }
        let readiness = match probe_ready(&target, None, &shell_quote(&place.ledger), None).await
        {
            Ok(readiness) => readiness,
            Err(refusal) => {
                refusals.push(refusal);
                continue;
            }
        };
        let router = service_endpoint(MODEL_ROUTER_SERVICE, &target.name).await?;
        let command = session_command(&Session {
            place: &place,
            origin: origin.as_deref(),
            target: &target.name,
            fleet_credentials: !readiness.reads_own_credentials,
            request: &request,
            router: router.as_deref(),
        });
        let run_id = run_id(kind, request.workspace);
        let options = SubmitOptions {
            batch_id: batch_id(kind),
            run_id: run_id.clone(),
            priority: request.priority,
            pinned_host: crate::cli::submit::resolve_pinned_host(&target.name).await?,
            cpu_cores: reservation.cpu_cores,
            memory_gb: reservation.ram_gb.ceil() as i64,
            // A host that holds the operator grant file reads the agent's
            // signing secret and the gateway bearer itself, and clones with
            // its own git credentials, exactly as an attached session does.
            // A host without it — the Linux builder has none — is handed the
            // same credentials by its own agent, from the fleet's grant, as
            // declared job secrets.
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
                gpu_mem_gb: Default::default(),
                gpu_type: String::new(),
                machine_type: CPU_MACHINE_TYPE.to_string(),
            }),
            ..SubmitOptions::default()
        };
        let jobs = submit_batch(std::slice::from_ref(&command), &options)
            .await
            .map_err(|error| {
                let message = format!("{kind} could not be queued on {}: {error}", target.name);
                let mut wrapped = CmdError::click(message);
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
        let cwd = place
            .checkout
            .clone()
            .unwrap_or_else(|| "its job's work tree".to_string());
        let record = json!({
            "schema_version": RECORD_SCHEMA_VERSION,
            "kind": kind,
            "target": target.name,
            "workspace": request.workspace,
            "cwd": cwd,
            "ledger": place.ledger,
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
            println!("{kind} queued on {} as job {} in {cwd}", target.name, job.job_id);
            println!(
                "holds {} core(s) and {} GiB; session ledger {}",
                reservation.cpu_cores, reservation.ram_gb, place.ledger
            );
            println!(
                "follow it with `stado job watch {} --follow-seconds <SECONDS>`, stop it with `stado cancel {}`",
                job.job_id, job.job_id
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

/// What one session command is built from.
struct Session<'a> {
    place: &'a Place,
    /// The origin the host's checkout is cloned from; none in the home
    /// workspace, which works in its own job tree.
    origin: Option<&'a str>,
    target: &'a str,
    /// Whether git reaches GitHub with the job's `GITHUB_TOKEN` rather than
    /// the host's own credentials.
    fleet_credentials: bool,
    request: &'a DetachedRequest<'a>,
    router: Option<&'a str>,
}

/// The one command the worker runs: the session's Cargo target and temporary
/// directories in the job tree, its ledger below the work root, the host's
/// checkout of the repository made ready and entered (the job tree itself in
/// the home workspace), then the managed Jeden with the grants the operator
/// asked for and nothing else. `--json` keeps the session's final answer
/// machine-readable in the job's retained output. Each preflight says on the
/// job's retained log what failed and what repairs it.
fn session_command(session: &Session<'_>) -> String {
    let request = session.request;
    let checkout = match (&session.place.checkout, session.origin) {
        (Some(checkout), Some(origin)) => format!(
            "{}{}",
            if session.fleet_credentials {
                github_token_environment()
            } else {
                String::new()
            },
            checkout_preamble(&session.place.checkouts, checkout, origin, session.target)
        ),
        _ => String::new(),
    };
    // A session with no resolved router leaves BRAMA_URL to Jeden's own
    // resolution on the host, which says itself which model route it took.
    let router_env = match session.router {
        Some(url) => format!("BRAMA_URL={} ", shell_quote(url)),
        None => String::new(),
    };
    let mut command = format!(
        "{}{}{checkout}\
         if [ ! -x \"$HOME\"/{MANAGED_JEDEN} ]; then printf 'the managed Jeden is missing or not executable at %s on this host; install it with `stado product install jeden`\\n' \"$HOME\"/{MANAGED_JEDEN} >&2; exit 1; fi; \
         {router_env}PATH=\"$HOME/.stado/bin:$PATH\" exec \"$HOME\"/{MANAGED_JEDEN} run {} --json --max-steps {}",
        job_tree_environment(),
        ledger_environment(session.place),
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
