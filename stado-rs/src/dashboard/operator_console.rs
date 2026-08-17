//! Authenticated, structured bridge from the operator dashboard to the finite
//! Stado CLI surface.
//!
//! The browser never submits a shell command. It submits an argv array, the
//! first element is checked against a closed command-family allowlist, and
//! mutating invocations require a second explicit confirmation value. This
//! keeps the GUI and CLI on one implementation instead of growing a second
//! control plane with different policy and recovery semantics.

use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const MAX_ARGUMENTS: usize = 96;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_REQUEST_BYTES: usize = MAX_INPUT_BYTES + (128 * 1024);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const MUTATION_CONFIRMATION: &str = "RUN_MUTATION";
const INPUT_PLACEHOLDER: &str = "$INPUT";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct StagedInput(std::path::PathBuf);

impl Drop for StagedInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const ALLOWED_FAMILIES: &[&str] = &[
    "alerts",
    "artifact",
    "azure",
    "billing",
    "blast-radius",
    "bootstrap",
    "cancel",
    "capabilities",
    "cloudflare",
    "config",
    "cost",
    "disk-cleanup",
    "doctor",
    "fleet",
    "host",
    "identity",
    "inference",
    "install-disk-cleanup",
    "instances",
    "job",
    "machine",
    "mail",
    "optimize",
    "overview",
    "placement",
    "profiles",
    "quota",
    "queue",
    "recovery",
    "registry",
    "release",
    "resources",
    "results",
    "schedule",
    "secrets",
    "service",
    "status",
    "storage",
    "submit",
    "vast",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunRequest {
    args: Vec<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    confirmation: String,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

#[derive(Debug)]
pub(super) struct ConsoleError {
    pub status: u16,
    pub message: String,
}

impl ConsoleError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: 403,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: message.into(),
        }
    }
}

fn action(id: &str, label: &str, description: &str, args: &[&str], input: Option<&str>) -> Value {
    let owned = args
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    json!({
        "id": id,
        "label": label,
        "description": description,
        "args": owned,
        "read_only": is_read_only(&owned),
        "input": input,
    })
}

fn section(id: &str, label: &str, description: &str, actions: Vec<Value>) -> Value {
    json!({
        "id": id,
        "label": label,
        "description": description,
        "actions": actions,
    })
}

/// Browser-visible catalog. Presets are editable argv templates, not a second
/// command implementation; advanced operators can use any allowlisted family.
pub(super) fn catalog() -> Value {
    json!({
        "schema_version": 1,
        "input_placeholder": INPUT_PLACEHOLDER,
        "mutation_confirmation": MUTATION_CONFIRMATION,
        "allowed_families": ALLOWED_FAMILIES,
        "sections": [
            section("jobs", "Jobs", "Inspect, submit, watch, rerun, cancel, and collect queued work.", vec![
                action("jobs-list", "List jobs", "Queue status and current job states.", &["status"], None),
                action("job-watch", "Watch job", "Follow one job's recorded output.", &["job", "watch", "JOB_ID"], None),
                action("job-rerun", "Rerun job", "Submit the exact recorded job specification again.", &["job", "rerun", "JOB_ID"], None),
                action("job-cancel", "Cancel job", "Cancel queued or running work.", &["cancel", "JOB_ID"], None),
                action("job-results", "Download results", "Copy one job's outputs to a host directory.", &["results", "JOB_ID", "OUTPUT_DIRECTORY"], None),
                action("machine-submit", "Submit machine request", "Submit a canonical JSON machine request from the input editor.", &["machine", "submit", "--request-file", INPUT_PLACEHOLDER], Some("Canonical machine request JSON")),
                action("machine-logs", "Read machine logs", "Read a bounded machine log page.", &["machine", "logs", "JOB_ID", "--limit", "200"], None),
                action("machine-artifacts", "Collect machine artifacts", "Download a machine job's declared artifacts.", &["machine", "artifacts", "JOB_ID", "OUTPUT_DIRECTORY"], None),
            ]),
            section("inference", "Inference", "Plan and operate model deployments, routes, reservations, and rollback.", vec![
                action("inference-status", "Deployment status", "Inspect deployed inference state.", &["inference", "status", "DEPLOYMENT_NAME", "--json"], None),
                action("inference-plan", "Plan yieldable deployment", "Persist a plan with an explicit KV-cache budget that yields to eligible queued GPU work.", &["inference", "plan", "DEPLOYMENT_NAME", "--host", "TARGET", "--image", "IMAGE", "--model", "MODEL", "--revision", "REVISION", "--gpu-mode", "yieldable", "--kv-cache-memory-gb", "KV_CACHE_MEMORY_GB"], None),
                action("inference-deploy", "Apply plan", "Apply a saved inference deployment plan.", &["inference", "apply", "PLAN_ID"], None),
                action("inference-rollback", "Rollback", "Restore the prior deployment generation.", &["inference", "rollback", "DEPLOYMENT_NAME"], None),
                action("inference-abort", "Abort plan", "Abort a pending deployment plan.", &["inference", "abort", "PLAN_ID"], None),
                action("inference-route", "Update route", "Compare-and-swap a logical inference route and ordered fallback.", &["inference", "route", "set", "ROUTE_ALIAS", "--to", "DEPLOYMENT_NAME", "--expected", "CURRENT_DESTINATION", "--fallback", "FALLBACK_ROUTE", "--json"], None),
                action("inference-retire", "Retire deployment", "Stop and forget a deployment while retaining its model cache.", &["inference", "retire", "DEPLOYMENT_NAME", "--json"], None),
            ]),
            section("queue", "Queue", "Pause, resume, drain, and explain scheduling decisions.", vec![
                action("queue-status", "Queue status", "Read maintenance and queue state.", &["queue", "status"], None),
                action("queue-pause", "Pause", "Pause new queue dispatch with an operator reason.", &["queue", "pause", "--reason", "OPERATOR_REASON"], None),
                action("queue-resume", "Resume", "Resume normal queue dispatch.", &["queue", "resume"], None),
                action("queue-drain", "Drain", "Drain accepted work without taking new work.", &["queue", "drain"], None),
            ]),
            section("hosts", "Hosts", "Inspect fleet health, capacity, versions, drift, cleanup, and recovery.", vec![
                action("host-health", "Health", "Read one host's health evidence.", &["host", "health", "TARGET", "--json"], None),
                action("host-inventory", "Inventory", "Read software, hardware, services, and capacity.", &["host", "inventory", "TARGET", "--json"], None),
                action("host-reconcile-preview", "Reconcile preview", "Show host drift without applying changes.", &["host", "reconcile", "--target", "TARGET", "--json"], None),
                action("host-reconcile", "Reconcile", "Apply declared host state.", &["host", "reconcile", "--target", "TARGET", "--apply", "--json"], None),
                action("host-recover", "Recover", "Run the declared host recovery path.", &["host", "recover", "TARGET"], None),
                action("host-cleanup", "Cleanup preview", "Inspect registry-controlled remote cleanup.", &["host", "cleanup", "TARGET", "--dry-run", "--json"], None),
            ]),
            section("services", "Services", "Inspect, deploy, relocate, restart, update, and retire managed services.", vec![
                action("service-list", "List services", "List declared managed services.", &["service", "list"], None),
                action("service-status", "Service status", "Read one service across its declared hosts.", &["service", "status", "SERVICE_NAME"], None),
                action("service-logs", "Service logs", "Read bounded service logs.", &["service", "logs", "SERVICE_NAME"], None),
                action("service-restart", "Restart", "Restart one declared service.", &["service", "restart", "SERVICE_NAME"], None),
                action("service-deploy", "Deploy", "Deploy the declared service artifact.", &["service", "deploy", "SERVICE_NAME"], None),
                action("service-retire", "Retire", "Retire a managed unit from one host.", &["service", "retire", "UNIT", "--host", "TARGET"], None),
                action("placement-move", "Relocate", "Move a declared service group between registered hosts.", &["placement", "move", "SERVICE_NAME", "--to-host", "TARGET"], None),
            ]),
            section("registry", "Registry", "Read, validate, compare-and-swap, and diagnose canonical fleet policy.", vec![
                action("registry-pull", "Pull document", "Print the complete canonical registry document.", &["registry", "pull"], None),
                action("registry-validate", "Validate document", "Validate JSON supplied in the input editor.", &["registry", "validate", INPUT_PLACEHOLDER], Some("Complete registry JSON")),
                action("registry-push", "Push document", "Publish a validated complete registry document.", &["registry", "push", INPUT_PLACEHOLDER], Some("Complete registry JSON")),
                action("registry-doctor", "Registry doctor", "Inspect registry authority and host identity.", &["registry", "doctor", "--json"], None),
                action("registry-beacons", "Beacon ages", "Inspect fleet heartbeat age.", &["registry", "beacon-age", "--json"], None),
            ]),
            section("artifacts", "Artifacts & releases", "Inspect, publish, promote, install, and roll back immutable software and job artifacts.", vec![
                action("artifact-list", "List artifacts", "List registered immutable artifacts.", &["artifact", "list"], None),
                action("artifact-publish", "Publish artifact", "Validate and atomically publish a manifest from the input editor.", &["artifact", "publish", INPUT_PLACEHOLDER, "--verify", "--json"], Some("Artifact manifest JSON")),
                action("artifact-alias", "Set alias", "Compare-and-swap a mutable alias to an immutable artifact.", &["artifact", "alias", "set", "TARGET_REF", "ALIAS", "--expected-previous", "CURRENT_REF", "--json"], None),
                action("artifact-show", "Show artifact", "Resolve an artifact reference and aliases.", &["artifact", "show", "ARTIFACT_REF"], None),
                action("release-status", "Release status", "Inspect current release state.", &["release", "status"], None),
                action("release-publish", "Submit release", "Submit a declared source artifact to the release pipeline.", &["release", "submit", "--source", "SOURCE", "--version", "VERSION"], None),
                action("release-promote", "Promote release", "Promote exact qualified candidate bytes to desired state.", &["release", "promote", "PRODUCT", "VERSION", "--channel", "stable", "--json"], None),
                action("release-rollback", "Rollback release", "Atomically restore the previous desired release.", &["release", "rollback", "PRODUCT", "--json"], None),
                action("host-release", "Install host release", "Move one host to a declared binary version.", &["host", "release", "TARGET", "--binary", "BINARY", "--version", "VERSION", "--json"], None),
            ]),
            section("schedules", "Schedules", "Create, inspect, pause, resume, run, and remove recurring jobs.", vec![
                action("schedule-create", "Create schedule", "Create a recurring job from a cron expression and command.", &["schedule", "create", "COMMAND", "--cron", "0 2 * * *", "--tz", "UTC"], None),
                action("schedule-list", "List schedules", "List recurring job definitions.", &["schedule", "list"], None),
                action("schedule-show", "Show schedule", "Inspect one recurring job.", &["schedule", "show", "SCHEDULE_ID"], None),
                action("schedule-run", "Run now", "Enqueue one schedule immediately.", &["schedule", "run", "SCHEDULE_ID"], None),
                action("schedule-pause", "Pause", "Pause one recurring job.", &["schedule", "pause", "SCHEDULE_ID"], None),
                action("schedule-resume", "Resume", "Resume one recurring job.", &["schedule", "resume", "SCHEDULE_ID"], None),
                action("schedule-remove", "Remove", "Remove one recurring job.", &["schedule", "rm", "SCHEDULE_ID"], None),
            ]),
            section("finops", "FinOps", "Inspect cost, quota, credits, grants, burn, and autonomous optimization.", vec![
                action("overview", "Operator overview", "Read jobs, workers, quota, budgets, burn, and credits.", &["overview", "--json"], None),
                action("billing-status", "Billing status", "Read cross-cloud balances and grants.", &["billing", "show"], None),
                action("cost-report", "Cost report", "Read attributed job and provider cost.", &["cost", "report"], None),
                action("quota-status", "Quota", "Read configured and available quota.", &["quota", "show", "--json"], None),
                action("optimize-status", "Optimizer status", "Inspect autonomous placement state.", &["optimize", "status"], None),
                action("optimize-run", "Run optimizer", "Run one policy-controlled optimization pass.", &["optimize", "run"], None),
            ]),
            section("recovery", "Recovery", "Inventory blast radius, plan repairs, execute fenced operations, and restore resources.", vec![
                action("blast-radius", "Blast radius", "Inventory one dependency's consumers and recovery coverage.", &["blast-radius", "DEPENDENCY"], None),
                action("resources-inventory", "Resource inventory", "Read managed resources, dependencies, and provider ownership.", &["resources", "show"], None),
                action("resource-operations", "Operation history", "List durable resource plans and recorded execution state.", &["resources", "operations", "list"], None),
                action("recovery-migrate", "Migrate storage", "Plan and execute a fenced provider migration.", &["recovery", "migrate", "--from", "SOURCE_PROVIDER", "--to", "TARGET_PROVIDER", "--enable-provider", "TARGET_PROVIDER"], None),
                action("storage-status", "Storage objects", "Inspect storage authority and objects.", &["storage", "ls"], None),
            ]),
            section("diagnostics", "Diagnostics", "Run ordered preflight, capability, identity, and fleet diagnostics.", vec![
                action("doctor", "Deployment doctor", "Run the ordered deployment preflight.", &["doctor", "--json"], None),
                action("capabilities", "Capability matrix", "Read capability families, providers, and selections.", &["capabilities", "--json"], None),
                action("registry-doctor-2", "Registry doctor", "Inspect canonical registry health.", &["registry", "doctor", "--json"], None),
                action("identity-list", "Identity inventory", "List declared workload identities.", &["identity", "list", "--json"], None),
            ]),
            section("alerts", "Alerts", "Inspect delivery state and manage policy-controlled alert channels.", vec![
                action("alerts-status", "Alert channels", "Read alert channel configuration and availability.", &["alerts", "channels"], None),
                action("alerts-send", "Send alert", "Send one policy-controlled operator alert.", &["alerts", "send", "MESSAGE"], None),
            ]),
            section("identity", "Credentials & identity", "Inspect identities and perform policy-controlled secret synchronization without exposing values in argv.", vec![
                action("identity-list-2", "List identities", "List identity contracts and consumers.", &["identity", "list", "--json"], None),
                action("identity-verify", "Verify identity", "Verify one workload identity contract.", &["identity", "verify", "--kind", "KIND", "--identity", "IDENTITY", "--json"], None),
                action("secrets-status", "Credential inventory", "List credential items without exposing values.", &["secrets", "ls"], None),
                action("secrets-doctor", "Credential doctor", "Inspect credential authority and synchronization.", &["secrets", "doctor"], None),
                action("service-env", "Service environment", "Inspect redacted managed service environment metadata.", &["service", "env", "SERVICE_NAME"], None),
                action("host-vaults", "Host vaults", "Inspect vault assignment without reading values.", &["host", "vaults", "TARGET", "--json"], None),
            ]),
            section("cloud", "Cloud providers", "Inspect and operate instances, quota, Azure, Cloudflare, and Vast through Stado policy.", vec![
                action("instances-list", "List instances", "Read provider instances known to Stado.", &["instances", "list"], None),
                action("vast-status", "Vast status", "Read Vast marketplace and managed rental state.", &["vast", "status"], None),
                action("vast-list", "List on Vast", "Publish the configured machine on the Vast marketplace.", &["vast", "list"], None),
                action("azure-status", "Azure deny diagnosis", "Inspect inherited system-protected UnusualActivity assignments.", &["azure", "unusual-activity", "diagnose"], None),
                action("cloudflare-route", "Route tunnel", "Apply a declared Cloudflare tunnel route and DNS record.", &["cloudflare", "route-tunnel", "--api-credential", "API_CREDENTIAL", "--tunnel-credential", "TUNNEL_CREDENTIAL", "--zone", "ZONE", "--hostname", "HOSTNAME", "--origin", "ORIGIN", "--host", "TARGET"], None),
                action("resources-inventory-2", "Cloud inventory", "Inventory declared cross-cloud resources.", &["resources", "show"], None),
            ]),
        ],
    })
}

fn read_pair(args: &[String]) -> (&str, &str) {
    (
        args.first().map(String::as_str).unwrap_or(""),
        args.get(1).map(String::as_str).unwrap_or(""),
    )
}

fn is_read_only(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return true;
    }
    let (family, operation) = read_pair(args);
    if family == "azure" && operation == "unusual-activity" {
        return args.get(2).map(String::as_str) == Some("diagnose");
    }
    if matches!(
        family,
        "capabilities"
            | "overview"
            | "profiles"
            | "status"
            | "doctor"
            | "results"
            | "cost"
            | "mail"
            | "blast-radius"
    ) {
        return true;
    }
    matches!(
        (family, operation),
        (
            "artifact",
            "list" | "show" | "resolve" | "verify" | "lineage"
        ) | ("billing", "show")
            // Enrollment mutates, and `fleet enroll`/`approve`/`key` are
            // gated as mutations on purpose. These five only read the
            // registry and the store; leaving them behind the confirmation
            // would teach the operator to type RUN_MUTATION to look at
            // something, which is the one habit this gate exists to prevent.
            | (
                "fleet",
                "list" | "status" | "catalog" | "doctor" | "pending"
            )
            | (
                "host",
                "health" | "inventory" | "uptime" | "ping" | "disk" | "vaults"
            )
            | ("identity", "list" | "verify")
            | (
                "inference",
                "list" | "status" | "logs" | "plan-logs" | "doctor" | "verify" | "blockers"
            )
            | ("instances", "list")
            | ("machine", "status" | "logs" | "artifacts")
            | ("optimize", "status" | "explain")
            | ("queue", "status")
            | ("quota", "show" | "catalog" | "requests" | "azure-replies")
            | (
                "registry",
                "validate" | "pull" | "self" | "doctor" | "beacon-age"
            )
            | ("release", "catalog" | "status")
            | ("resources", "show" | "verify" | "operations")
            | ("schedule", "list" | "show")
            | ("secrets", "ls" | "doctor" | "inspect-vault")
            | (
                "service",
                "directory"
                    | "list"
                    | "onboarding-catalog"
                    | "status"
                    | "show"
                    | "logs"
                    | "env"
                    | "auth-check"
            )
            | (
                "storage",
                "ls" | "stat" | "cat" | "verify" | "objects" | "url"
            )
            | ("vast", "status")
            | ("alerts", "channels")
    )
}

fn validate_request(request: &RunRequest) -> Result<(), ConsoleError> {
    if request.args.is_empty() || request.args.len() > MAX_ARGUMENTS {
        return Err(ConsoleError::bad_request(format!(
            "args must contain 1 to {MAX_ARGUMENTS} values"
        )));
    }
    if request
        .args
        .iter()
        .any(|arg| arg.is_empty() || arg.len() > MAX_ARGUMENT_BYTES || arg.contains('\0'))
    {
        return Err(ConsoleError::bad_request(
            "arguments must be non-empty, bounded strings without NUL bytes",
        ));
    }
    let family = request.args[0].as_str();
    if !ALLOWED_FAMILIES.contains(&family) {
        return Err(ConsoleError::forbidden(format!(
            "command family {family:?} is not available in the dashboard"
        )));
    }
    if request.timeout_seconds == 0 || request.timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(ConsoleError::bad_request(format!(
            "timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}"
        )));
    }
    if request.input.as_ref().map_or(0, String::len) > MAX_INPUT_BYTES {
        return Err(ConsoleError::bad_request(format!(
            "input exceeds the {MAX_INPUT_BYTES}-byte limit"
        )));
    }
    let needs_input = request.args.iter().any(|arg| arg == INPUT_PLACEHOLDER);
    if needs_input && request.input.is_none() {
        return Err(ConsoleError::bad_request(
            "$INPUT requires content in the input editor",
        ));
    }
    if !is_read_only(&request.args) && request.confirmation != MUTATION_CONFIRMATION {
        return Err(ConsoleError::forbidden(
            "mutating commands require explicit RUN_MUTATION confirmation",
        ));
    }
    Ok(())
}

fn temporary_input_path() -> std::path::PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "stado-dashboard-input-{}-{now}-{sequence}.json",
        std::process::id()
    ))
}

fn parse_structured_output(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}
async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(16 * 1024);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
        let retained = remaining.min(read);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((output, truncated))
}

/// Execute one finite, authenticated CLI action and return bounded captured
/// output. `current_exe` preserves the exact dashboard release and therefore
/// the exact command contracts that rendered the catalog.
pub(super) async fn run(body: &[u8]) -> Result<Value, ConsoleError> {
    let request: RunRequest = serde_json::from_slice(body)
        .map_err(|error| ConsoleError::bad_request(format!("invalid JSON: {error}")))?;
    validate_request(&request)?;

    let temporary = if request.args.iter().any(|arg| arg == INPUT_PLACEHOLDER) {
        let path = temporary_input_path();
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(&path).await.map_err(|error| {
            ConsoleError::unavailable(format!("could not stage input: {error}"))
        })?;
        file.write_all(request.input.as_deref().unwrap_or_default().as_bytes())
            .await
            .map_err(|error| {
                ConsoleError::unavailable(format!("could not stage input: {error}"))
            })?;
        drop(file);
        Some(StagedInput(path))
    } else {
        None
    };

    let mut args = request.args.clone();
    if let Some(path) = &temporary {
        let path = path.0.to_string_lossy().into_owned();
        for arg in &mut args {
            if arg == INPUT_PLACEHOLDER {
                *arg = path.clone();
            }
        }
    }

    let executable = std::env::current_exe().map_err(|error| {
        ConsoleError::unavailable(format!("could not resolve Stado binary: {error}"))
    })?;
    let mut command = Command::new(executable);
    command
        .args(&args)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("STADO_DASHBOARD_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        ConsoleError::unavailable(format!("could not start Stado command: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConsoleError::unavailable("could not capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ConsoleError::unavailable("could not capture command stderr"))?;
    let execution = async {
        let (stdout_result, stderr_result, status_result) =
            tokio::join!(read_bounded(stdout), read_bounded(stderr), child.wait(),);
        let (stdout_bytes, stdout_truncated) = stdout_result.map_err(|error| {
            ConsoleError::unavailable(format!("could not read command stdout: {error}"))
        })?;
        let (stderr_bytes, stderr_truncated) = stderr_result.map_err(|error| {
            ConsoleError::unavailable(format!("could not read command stderr: {error}"))
        })?;
        let status = status_result.map_err(|error| {
            ConsoleError::unavailable(format!("could not wait for Stado command: {error}"))
        })?;
        Ok::<_, ConsoleError>((
            status,
            stdout_bytes,
            stderr_bytes,
            stdout_truncated,
            stderr_truncated,
        ))
    };
    let result =
        tokio::time::timeout(Duration::from_secs(request.timeout_seconds), execution).await;
    let (status, stdout_bytes, stderr_bytes, stdout_truncated, stderr_truncated) = match result {
        Ok(result) => result?,
        Err(_) => {
            return Err(ConsoleError::unavailable(format!(
                "command exceeded the {} second dashboard limit",
                request.timeout_seconds
            )))
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let structured = parse_structured_output(&stdout);
    Ok(json!({
        "ok": status.success(),
        "exit_code": status.code(),
        "read_only": is_read_only(&request.args),
        "args": request.args,
        "stdout": stdout,
        "stderr": stderr,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated,
        "structured": structured,
    }))
}
