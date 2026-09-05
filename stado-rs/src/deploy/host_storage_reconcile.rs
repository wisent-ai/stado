//! Interruption-safe reconciliation of the two fixed co-located local object roots.
//!
//! This is deliberately not `host object-relocate`: relocation moves one in-store
//! address and refuses overwrites. This transaction checkpoints both physical
//! roots with copy-on-write clones, then additively makes `local-storage`
//! contain `local-backup`'s exact objects and effective metadata. Backup bytes
//! and primary-only objects are never removed. The immutable full-primary
//! checkpoint retains conflicting primary bytes before the backup-winning
//! value is installed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use super::{host_channel, service};
use super::{shlex_quote, DeployError, Runner};
use base64::Engine;
use sha2::{Digest, Sha256};

pub const CHECKPOINT: &str = "checkpoint";
pub const APPLY: &str = "apply";
pub const FINALIZE: &str = "finalize";
pub const ACTIVATE: &str = "activate";
pub const ROLLBACK: &str = "rollback";
pub const STATUS: &str = "status";
pub const RUN: &str = "run";
pub const RESUME: &str = "resume";
const TIMEOUT: Duration = Duration::from_secs(60 * 60);
const PREFLIGHT: &str = "preflight";
const ARM_ACTIVATION: &str = "arm-activation";
const ARM_ROLLBACK: &str = "arm-rollback";
const RECORD_LIFECYCLE_DECISIONS: &str = "record-lifecycle-decisions";
static RESIDENT_OWNER_TOKEN: OnceLock<String> = OnceLock::new();
static RESIDENT_RUNNER_GATE: OnceLock<Value> = OnceLock::new();
static RESIDENT_LOCK_FD: OnceLock<i32> = OnceLock::new();
static RESIDENT_TARGET: OnceLock<crate::targets::ComputeTarget> = OnceLock::new();
static RESIDENT_NATIVE_MANAGER: OnceLock<Value> = OnceLock::new();
const ROLLBACK_OBJECT_API_SCRIPT: &str = r#"set -euo pipefail
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'unsupported_os\n' >&2
  exit 65
fi
label=com.wisent.always-on.stado-object-api
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
store=@PRIMARY@
backup_backend=@BACKUP_BACKEND@
backup_store=@BACKUP@
config=@CONFIG@
port=@PORT@
log="$HOME/.stado/logs/$label.log"
work="$HOME/.stado/work/object-api-recovery"
[ -x "$program" ] && [ -d "$store" ] && [ -r "$store/registry.json" ]
if [ -n "$backup_store" ]; then
  [ "$backup_backend" = local ] && [ -d "$backup_store" ] && [ -r "$backup_store/registry.json" ]
fi
/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.captured-prior.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)
/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$backup_backend" "$backup_store" "$account" "$log" "$HOME" "$config" "$port" <<'PY'
import plistlib, sys
path, label, program, store, backup_backend, backup_store, account, log, home, config, port = sys.argv[1:]
environment = {
    "HOME": home,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "STADO_CONFIG": config,
    "GNUPGHOME": f"{home}/.gnupg",
    "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
    "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
    "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
    "WC_STORAGE_BACKEND": "local",
    "WC_LOCAL_STORAGE_PATH": store,
}
if backup_store:
    environment["WC_BACKUP_STORAGE_BACKEND"] = backup_backend
    environment["WC_BACKUP_LOCAL_STORAGE_PATH"] = backup_store
document = {
    "Label": label,
    "ProgramArguments": [program, "dashboard", "--bind", "127.0.0.1", "--port", port],
    "EnvironmentVariables": environment,
    "RunAtLoad": True,
    "KeepAlive": True,
    "UserName": account,
    "StandardOutPath": log,
    "StandardErrorPath": log,
}
with open(path, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
PY
/usr/bin/plutil -lint "$staged" >/dev/null
/usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl enable "system/$label"
/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
printf 'STADO_OBJECT_API_ROUTE\tcaptured-prior\n'
"#;

const REMOTE_PYTHON: &str = include_str!("host_storage_reconcile.py");

const FENCE_SCHEMA: &str = "stado.storage-root-fence.v5";
const READ_FENCE: &str = "read-fence";
const READ_OWNER: &str = "read-owner";
const PREFLIGHT_EVIDENCE_FILE: &str = "preflight.json";
const CHECKPOINT_EVIDENCE_FILE: &str = "checkpoint-evidence.json";
const LIFECYCLE_DECISIONS_FILE: &str = "lifecycle-decisions.json";
const FINAL_LIFECYCLE_OBSERVATIONS_FILE: &str = "final-lifecycle-observations.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileSnapshot {
    body_base64: String,
    sha256: String,
    mode: u32,
    uid: u32,
    gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedScript {
    body: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriterFence {
    target: String,
    label: String,
    role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    storage_evidence: Vec<String>,
    path: String,
    listener_port: Option<u16>,
    was_loaded: bool,
    was_runnable: bool,
    loaded_domains: Vec<String>,
    autostart: BTreeMap<String, bool>,
    prior_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prior_started_at: Option<String>,
    prior_loaded_environment: BTreeMap<String, String>,
    registry_declared_environment: BTreeMap<String, String>,
    unit_declared_environment: BTreeMap<String, String>,
    prior_executable: Option<String>,
    prior_sha256: Option<String>,
    prior_device: Option<u64>,
    prior_inode: Option<u64>,
    unit_snapshot: Option<FileSnapshot>,
    prior_native_state: Option<String>,
    prior_last_exit_code: Option<String>,
    prior_restart: Option<String>,
    prior_triggers: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forward_object_recovery: Option<PreparedScript>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollback_object_recovery: Option<PreparedScript>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    restored_loaded_environment: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_inode: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_route: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueEffect {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_content: Option<String>,
    intended: crate::queue::control::QueueControl,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseding: Option<crate::queue::control::QueueControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueFence {
    was_paused: bool,
    drained: bool,
    resumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause: Option<QueueEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restoration: Option<QueueEffect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseAcquisition {
    subject_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease: Option<crate::autonomy::storage::PlacementLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    released_lease: Option<crate::autonomy::storage::PlacementLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ImmutableEvidenceReference {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageRoots {
    primary: String,
    backup: String,
    prior_primary: String,
    prior_backup: Option<String>,
    runtime: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriteFenceEffect {
    status: String,
    intent: Value,
    acquired_at: Option<i64>,
    released_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleFence {
    schema: String,
    transaction: String,
    resident_owner: Value,
    status: String,
    queue: QueueFence,
    writers: Vec<WriterFence>,
    transport_retained: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    non_storage_retained: Vec<Value>,
    staged_runtime: Option<super::host_release::StagedRelease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    roots: Option<StorageRoots>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_fence: Option<WriteFenceEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preflight_evidence: Option<ImmutableEvidenceReference>,
    #[serde(default)]
    rollback_preparation: bool,
    #[serde(default)]
    lease_acquisitions: Vec<LeaseAcquisition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository_runner_gate: Option<Value>,
    prepared_at: i64,
    rechecked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    activated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_at: Option<i64>,
}

fn bind_remote_script(phase: &str, transaction: &str) -> String {
    let mut script = String::with_capacity(REMOTE_PYTHON.len() + 512);
    script.push_str("set -u\nSTADO_RECONCILE_PHASE=");
    script.push_str(&shlex_quote(phase));
    script.push_str(" STADO_RECONCILE_TX=");
    script.push_str(&shlex_quote(transaction));
    script.push_str(" STADO_RECONCILE_OWNER_TOKEN=");
    script.push_str(&shlex_quote(
        RESIDENT_OWNER_TOKEN.get().map(String::as_str).unwrap_or(""),
    ));
    script.push_str(" STADO_RECONCILE_LOCK_FD=");
    script.push_str(&shlex_quote(
        &RESIDENT_LOCK_FD.get().copied().unwrap_or(-1).to_string(),
    ));
    script.push_str(" /usr/bin/python3 - 2>&1 <<'STADO_RECONCILE_EOF'\n");
    script.push_str(REMOTE_PYTHON);
    if !REMOTE_PYTHON.ends_with('\n') {
        script.push('\n');
    }
    script.push_str("STADO_RECONCILE_EOF\n");
    script
}

fn remote_failure_detail(output: &super::CommandOutput, fallback: &str) -> String {
    let stdout = output.stdout.trim();
    let stderr = output.stderr.trim();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => fallback.to_string(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn parse_remote_payload(output: &super::CommandOutput) -> Result<Value, DeployError> {
    let mut payload = None;
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        if let Some(encoded) = line.strip_prefix("STADO_STORAGE_RECONCILE\t") {
            payload = serde_json::from_str(encoded).ok();
        }
    }
    if !output.ok() {
        return Err(DeployError(remote_failure_detail(
            output,
            "storage reconciliation host program failed",
        )));
    }
    payload.ok_or_else(|| DeployError("storage reconciliation returned no payload".to_string()))
}

async fn read_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<LifecycleFence>, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(READ_FENCE, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    let value = parse_remote_payload(&output)?;
    if value.get("status").and_then(Value::as_str) == Some("absent") {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| DeployError(format!("invalid durable lifecycle fence: {error}")))
}

async fn write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &LifecycleFence,
    _runner: &Runner,
) -> Result<(), DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "lifecycle fence can only be written by the resident target worker".to_string(),
        ));
    }
    if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
        return Err(DeployError(
            "lifecycle fence belongs to another transaction".to_string(),
        ));
    }
    verify_resident_lock(transaction)?;
    atomic_json_file(
        &transaction_directory(transaction)?.join("lifecycle-fence.json"),
        fence,
        "lifecycle fence",
    )
}
async fn refresh_resident_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let current = resident_owner_retention(transaction)?;
    if fence.resident_owner != current {
        fence.resident_owner = current;
        write_fence(target, transaction, fence, runner).await?;
    }
    Ok(())
}

async fn repository_runner_gate() -> Result<Option<Value>, DeployError> {
    if let Some(gate) = RESIDENT_RUNNER_GATE.get() {
        return Ok(Some(gate.clone()));
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Ok(None);
    }
    let required = |name: &str| {
        std::env::var(name).map_err(|_| {
            DeployError(format!(
                "{name} is required when storage reconciliation owns an Actions runner"
            ))
        })
    };
    let repository = required("GITHUB_REPOSITORY")?;
    let owner = repository
        .split_once('/')
        .map(|(owner, _)| owner)
        .filter(|owner| !owner.is_empty())
        .ok_or_else(|| DeployError("GITHUB_REPOSITORY is not owner/repository".to_string()))?;
    let current_runner = required("RUNNER_NAME")?;
    let run_id = required("GITHUB_RUN_ID")?;
    let source_sha = required("GITHUB_SHA")?;
    let token = super::host_precheck_runner::github_credential().await?;
    let client = reqwest::Client::new();
    let request = |endpoint: String| {
        client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&token)
    };

    let run_endpoint = format!("https://api.github.com/repos/{repository}/actions/runs/{run_id}");
    let run_response = request(run_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow run: {error}")))?;
    if !run_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow run returned HTTP {}",
            run_response.status()
        )));
    }
    let run: Value = run_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow run: {error}")))?;
    if run
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        != Some(run_id.clone())
        || run.get("head_sha").and_then(Value::as_str) != Some(source_sha.as_str())
        || !matches!(
            run.get("status").and_then(Value::as_str),
            Some("in_progress" | "queued")
        )
    {
        return Err(DeployError(
            "GitHub run identity does not match this source invocation".to_string(),
        ));
    }

    let jobs_endpoint = format!(
        "https://api.github.com/repos/{repository}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"
    );
    let jobs_response = request(jobs_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow jobs: {error}")))?;
    if !jobs_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow jobs returned HTTP {}",
            jobs_response.status()
        )));
    }
    let jobs: Value = jobs_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow jobs: {error}")))?;
    let job_rows = jobs
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("current workflow jobs omitted jobs".to_string()))?;
    if jobs.get("total_count").and_then(Value::as_u64) != Some(job_rows.len() as u64) {
        return Err(DeployError(
            "current workflow jobs response was paginated or incomplete".to_string(),
        ));
    }
    let executing = job_rows
        .iter()
        .filter(|job| {
            job.get("runner_name").and_then(Value::as_str) == Some(current_runner.as_str())
                && job.get("status").and_then(Value::as_str) == Some("in_progress")
        })
        .collect::<Vec<_>>();
    if executing.len() != 1 {
        return Err(DeployError(format!(
            "expected one in-progress job on runner {current_runner:?}, found {}",
            executing.len()
        )));
    }
    let current_job = executing[0];
    let current_runner_id = current_job
        .get("runner_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted runner_id".to_string()))?;
    let current_job_id = current_job
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted id".to_string()))?;

    let repositories = [
        repository.clone(),
        format!("{owner}/wisent-backend"),
        format!("{owner}/brama"),
    ];
    let mut current_online_busy = false;
    let mut other_busy = Vec::new();
    let mut inventory = Vec::new();
    for repository_name in &repositories {
        let endpoint =
            format!("https://api.github.com/repos/{repository_name}/actions/runners?per_page=100");
        let response = request(endpoint).send().await.map_err(|error| {
            DeployError(format!(
                "cannot read runners for {repository_name}: {error}"
            ))
        })?;
        if !response.status().is_success() {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} returned HTTP {}",
                response.status()
            )));
        }
        let body: Value = response.json().await.map_err(|error| {
            DeployError(format!(
                "invalid runner inventory for {repository_name}: {error}"
            ))
        })?;
        let runners = body
            .get("runners")
            .and_then(Value::as_array)
            .ok_or_else(|| DeployError(format!("{repository_name} omitted runners")))?;
        if body.get("total_count").and_then(Value::as_u64) != Some(runners.len() as u64) {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} was paginated or incomplete"
            )));
        }
        for runner_row in runners {
            let id = runner_row.get("id").and_then(Value::as_u64);
            let name = runner_row
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let busy = runner_row.get("busy").and_then(Value::as_bool) == Some(true);
            let online = runner_row.get("status").and_then(Value::as_str) == Some("online");
            if id == Some(current_runner_id) {
                current_online_busy |= online && busy;
            } else if busy {
                other_busy.push(json!({"repository": repository_name, "id": id, "name": name}));
            }
            inventory.push(json!({
                "repository": repository_name,
                "id": id,
                "name": name,
                "online": online,
                "busy": busy,
            }));
        }
    }
    if !current_online_busy || !other_busy.is_empty() {
        return Err(DeployError(format!(
            "fleet runner fence refused: current_online_busy={current_online_busy}, other_busy={other_busy:?}"
        )));
    }
    Ok(Some(json!({
        "repositories": repositories,
        "current_repository": repository,
        "current_run_id": run_id,
        "current_job_id": current_job_id,
        "current_job_name": current_job.get("name"),
        "current_runner": current_runner,
        "current_runner_id": current_runner_id,
        "current_online_busy": true,
        "other_busy": other_busy,
        "inventory": inventory,
        "source_sha": source_sha,
        "checked_at": Utc::now().timestamp(),
    })))
}

#[derive(Debug, Clone)]
struct ServiceCandidate {
    target: crate::targets::ComputeTarget,
    declared: service::ManagedService,
    loaded_domains: Vec<String>,
    observed_command: String,
    storage_evidence: BTreeSet<String>,
}

fn command_tokens(command: &str) -> Vec<&str> {
    command
        .split_ascii_whitespace()
        .map(|token| token.trim_matches(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | ',' | '"')))
        .filter(|token| !token.is_empty())
        .collect()
}

fn executable_name(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

fn storage_route_key(key: &str) -> bool {
    matches!(
        key,
        "STADO_CONFIG"
            | "WC_STORAGE_BACKEND"
            | "WC_LOCAL_STORAGE_PATH"
            | "WC_BACKUP_STORAGE_BACKEND"
            | "WC_BACKUP_LOCAL_STORAGE_PATH"
            | "WC_STADO_STORAGE_URL"
            | "WC_STADO_STORAGE_NAMESPACE"
            | "WC_STADO_STORAGE_TOKEN_FILE"
    )
}

fn service_role(label: &str, command: &str) -> &'static str {
    const OBJECT_API_LABEL: &str = "com.wisent.always-on.stado-object-api";
    if label == OBJECT_API_LABEL {
        return "object-api";
    }
    let tokens = command_tokens(command);
    if tokens
        .iter()
        .any(|token| executable_name(token) == "Runner.Listener")
    {
        return "runner";
    }
    let executable = tokens
        .iter()
        .position(|token| executable_name(token) == "stado");
    if executable.is_some_and(|index| {
        tokens.get(index + 1).copied() == Some("release")
            && tokens.get(index + 2).copied() == Some("agent")
    }) {
        return "release-agent";
    }
    if let Some(index) = executable {
        return match tokens.get(index + 1).copied() {
            Some("resolver") => "transport",
            Some("coordinator" | "local-control-plane" | "cloud-control-plane") => "coordinator",
            Some("agent") => "agent",
            Some("disk-cleanup") => "disk-cleanup",
            _ => "writer",
        };
    }
    match tokens
        .first()
        .map(|token| executable_name(token))
        .unwrap_or_default()
    {
        "caddy" | "cloudflared" | "tailscaled" | "skarbiec" | "skarbiec-control-plane" | "ssh" => {
            "transport"
        }
        "stado-fix" => "agent",
        _ => "other",
    }
}

fn managed_from_unit(
    target: &crate::targets::ComputeTarget,
    label: &str,
    path: &str,
    kind: &str,
) -> service::ManagedService {
    if kind == service::KIND_SYSTEMD {
        service::systemd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    } else {
        service::launchd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    }
}

fn exact_identity_component(value: &str, identity: &str) -> bool {
    value
        .split(['.', '/', '\\'])
        .any(|component| component == identity)
}

fn current_runner_candidate(
    candidate: &ServiceCandidate,
    command: &str,
    current_runner: &str,
) -> bool {
    exact_identity_component(candidate.declared.unit_id(), current_runner)
        || exact_identity_component(&candidate.declared.path, current_runner)
        || command_tokens(command).contains(&current_runner)
}

fn resident_owner_retention(transaction: &str) -> Result<Value, DeployError> {
    verify_resident_lock(transaction)?;
    let identity = RESIDENT_NATIVE_MANAGER.get().ok_or_else(|| {
        DeployError("resident native manager identity was not initialized".to_string())
    })?;
    let expected_service = if cfg!(target_os = "linux") {
        format!("com.wisent.stado-storage-root-reconcile.{transaction}.service")
    } else {
        format!("com.wisent.stado-storage-root-reconcile.{transaction}")
    };
    if identity.get("service").and_then(Value::as_str) != Some(expected_service.as_str())
        || identity.get("pid").and_then(Value::as_u64) != Some(u64::from(std::process::id()))
    {
        return Err(DeployError(
            "resident native manager identity does not bind this exact transaction process"
                .to_string(),
        ));
    }
    let lock_path = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| DeployError("transaction directory has no authority root".to_string()))?
        .join("storage-root-reconcile.lock");
    let lock = std::fs::metadata(&lock_path)
        .map_err(|error| DeployError(format!("cannot inspect resident lock identity: {error}")))?;
    Ok(json!({
        "role": "resident-transaction-owner",
        "native_manager": identity,
        "process_pid": std::process::id(),
        "lock_device": lock.dev(),
        "lock_inode": lock.ino(),
    }))
}

async fn registry_services(
    storage_target: &crate::targets::ComputeTarget,
    resident_owner_unit: &str,
    runner: &Runner,
) -> Result<Vec<ServiceCandidate>, DeployError> {
    let mut candidates = BTreeMap::<String, ServiceCandidate>::new();
    for declared in service::declared_services(storage_target) {
        if declared.unit_id() == resident_owner_unit {
            continue;
        }
        let storage_evidence = declared
            .env
            .keys()
            .filter(|key| storage_route_key(key))
            .cloned()
            .collect();
        candidates.insert(
            declared.unit_id().to_string(),
            ServiceCandidate {
                target: storage_target.clone(),
                observed_command: std::iter::once(declared.program.as_str())
                    .chain(declared.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
                declared,
                loaded_domains: Vec::new(),
                storage_evidence,
            },
        );
    }
    for product in super::products::declared()? {
        if !storage_target.managed_versions.contains_key(&product.name) {
            continue;
        }
        for unit in &product.units {
            let label = unit.label_for(&storage_target.name);
            if label == resident_owner_unit {
                continue;
            }
            let Some(path) = unit.path_for(&storage_target.name) else {
                continue;
            };
            let kind = unit.kind.as_deref().unwrap_or(service::KIND_LAUNCHD);
            candidates
                .entry(label.clone())
                .or_insert_with(|| ServiceCandidate {
                    target: storage_target.clone(),
                    declared: managed_from_unit(storage_target, &label, &path, kind),
                    loaded_domains: Vec::new(),
                    observed_command: String::new(),
                    storage_evidence: BTreeSet::new(),
                });
        }
    }
    let mut resident_owner_discovered = false;
    for native in service::loaded_units(storage_target, runner).await? {
        let label = native.label.clone();
        if label == resident_owner_unit {
            if native.pid.parse::<u32>().ok() != Some(std::process::id()) {
                return Err(DeployError(
                    "loaded-unit scan did not bind the exact resident owner service to this process"
                        .to_string(),
                ));
            }
            resident_owner_discovered = true;
            candidates.remove(&label);
            continue;
        }
        let candidate = candidates.entry(label.clone()).or_insert_with(|| {
            let kind = if native.path.ends_with(".service") {
                service::KIND_SYSTEMD
            } else {
                service::KIND_LAUNCHD
            };
            ServiceCandidate {
                target: storage_target.clone(),
                declared: managed_from_unit(storage_target, &label, &native.path, kind),
                loaded_domains: Vec::new(),
                observed_command: String::new(),
                storage_evidence: BTreeSet::new(),
            }
        });
        for key in native
            .env_keys
            .iter()
            .chain(&native.script_reads)
            .chain(&native.script_assigns)
            .filter(|key| storage_route_key(key))
        {
            candidate.storage_evidence.insert(key.clone());
        }
        if candidate.declared.path.is_empty() && !native.path.is_empty() {
            candidate.declared.path.clone_from(&native.path);
        }
        candidate.loaded_domains = native.loaded_domains;
        if !native.running_program.is_empty() {
            candidate.observed_command = native.running_program;
        } else if candidate.observed_command.is_empty() {
            candidate.observed_command = native.program;
        }
    }
    if !resident_owner_discovered {
        return Err(DeployError(
            "loaded-unit scan omitted the exact resident transaction owner".to_string(),
        ));
    }
    Ok(candidates.into_values().collect())
}

fn command_u16_option(command: &str, option: &str) -> Option<u16> {
    let tokens = command_tokens(command);
    tokens
        .windows(2)
        .find(|pair| pair[0] == option)
        .and_then(|pair| pair[1].parse().ok())
}

fn stop_priority(role: &str) -> u8 {
    match role {
        "runner" => 0,
        "current-runner" => 1,
        "release-agent" => 2,
        "coordinator" => 3,
        "agent" | "disk-cleanup" => 4,
        "object-api" => u8::MAX,
        _ => 5,
    }
}

async fn renew_fence_leases(
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
) -> Result<(), DeployError> {
    if fence
        .write_fence
        .as_ref()
        .is_some_and(|effect| matches!(effect.status.as_str(), "acquired" | "release_intent"))
    {
        return Ok(());
    }
    const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
    for acquisition in &mut fence.lease_acquisitions {
        if matches!(
            acquisition.status.as_str(),
            "release_intent" | "released" | "superseded"
        ) {
            continue;
        }
        if acquisition.status != "acquired" {
            return Err(DeployError(format!(
                "placement lease {} has non-renewable state {:?}",
                acquisition.subject_id, acquisition.status
            )));
        }
        let lease = acquisition.lease.as_mut().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
        })?;
        let renewed = crate::autonomy::storage::renew_placement_lease(
            store,
            &lease.subject_id,
            &lease.token,
            LEASE_TTL_SECONDS,
            Utc::now(),
        )
        .await
        .map_err(|error| DeployError(format!("cannot renew {}: {error}", lease.subject_id)))?;
        *lease = match renewed {
            Some(renewed) => renewed,
            None => crate::autonomy::storage::acquire_placement_lease(
                store,
                &lease.subject_id,
                &fence.transaction,
                &lease.holder,
                LEASE_TTL_SECONDS,
                Utc::now(),
            )
            .await
            .map_err(|error| {
                DeployError(format!(
                    "cannot recover lease {}: {error}",
                    lease.subject_id
                ))
            })?
            .ok_or_else(|| {
                DeployError(format!(
                    "placement lease ownership changed for {}",
                    lease.subject_id
                ))
            })?,
        };
    }
    Ok(())
}

enum QueueEffectOutcome {
    Applied,
    Superseded(crate::queue::control::QueueControl),
}

fn parse_queue_control(
    content: Option<&str>,
) -> Result<crate::queue::control::QueueControl, DeployError> {
    match content {
        None => Ok(crate::queue::control::QueueControl::default()),
        Some(content) if content.trim().is_empty() => {
            Ok(crate::queue::control::QueueControl::default())
        }
        Some(content) => serde_json::from_str(content)
            .map_err(|error| DeployError(format!("queue control is invalid: {error}"))),
    }
}

async fn execute_queue_effect(
    store: &crate::queue::JobStorage,
    effect: &QueueEffect,
) -> Result<QueueEffectOutcome, DeployError> {
    let current = store
        .read_text_versioned(crate::queue::control::CONTROL_BLOB)
        .await
        .map_err(|error| DeployError(format!("cannot read queue transition state: {error}")))?;
    let intended = effect.intended.to_json();
    if current
        .as_ref()
        .is_some_and(|versioned| versioned.content == intended)
    {
        return Ok(QueueEffectOutcome::Applied);
    }
    let expected_matches = match (&current, &effect.expected_version, &effect.expected_content) {
        (None, None, None) => true,
        (Some(current), Some(version), Some(content)) => {
            current.version == *version && current.content == *content
        }
        _ => false,
    };
    if !expected_matches {
        return parse_queue_control(current.as_ref().map(|value| value.content.as_str()))
            .map(QueueEffectOutcome::Superseded);
    }
    let write = match current {
        Some(current) => store
            .compare_and_swap_text(
                crate::queue::control::CONTROL_BLOB,
                &current.version,
                &intended,
            )
            .await
            .map(|_| true),
        None => {
            store
                .create_text_if_absent(crate::queue::control::CONTROL_BLOB, &intended)
                .await
        }
    };
    match write {
        Ok(true) => Ok(QueueEffectOutcome::Applied),
        Ok(false)
        | Err(crate::queue::StorageError::StorageConflict(_))
        | Err(crate::queue::StorageError::NotFound(_)) => Err(DeployError(
            "queue control changed during its recorded conditional transition".to_string(),
        )),
        Err(error) => Err(DeployError(format!(
            "cannot apply recorded queue transition: {error}"
        ))),
    }
}

async fn release_fence_leases(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    for index in 0..fence.lease_acquisitions.len() {
        let status = fence.lease_acquisitions[index].status.as_str();
        if matches!(status, "released" | "superseded") {
            continue;
        }
        if status == "acquired" {
            let mut released = fence.lease_acquisitions[index]
                .lease
                .clone()
                .ok_or_else(|| {
                    DeployError(format!(
                        "placement lease acquisition for {} has no durable result",
                        fence.lease_acquisitions[index].subject_id
                    ))
                })?;
            released.expires_at = Utc::now().to_rfc3339();
            fence.lease_acquisitions[index].released_lease = Some(released);
            fence.lease_acquisitions[index].status = "release_intent".to_string();
            write_fence(storage_target, transaction, fence, runner).await?;
        } else if status != "release_intent" {
            return Err(DeployError(format!(
                "placement lease {} has non-releasable state {:?}",
                fence.lease_acquisitions[index].subject_id, status
            )));
        }
        let acquisition = &fence.lease_acquisitions[index];
        let owned = acquisition.lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
        })?;
        let released = acquisition.released_lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease release for {} has no durable intended result",
                acquisition.subject_id
            ))
        })?;
        let relinquished =
            crate::autonomy::storage::release_placement_lease_exact(store, owned, released)
                .await
                .map_err(|error| {
                    DeployError(format!(
                        "cannot release placement lease {}: {error}",
                        acquisition.subject_id
                    ))
                })?;
        fence.lease_acquisitions[index].status = if relinquished {
            "released"
        } else {
            "superseded"
        }
        .to_string();
        write_fence(storage_target, transaction, fence, runner).await?;
    }
    Ok(())
}

async fn restore_queue_control(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
    rollback: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    if fence.queue.was_paused {
        fence.queue.resumed = true;
        return Ok(());
    }
    if fence.queue.restoration.is_none() {
        let owned = fence
            .queue
            .pause
            .as_ref()
            .filter(|effect| effect.status == "applied")
            .ok_or_else(|| {
                DeployError("queue restoration has no exact owned pause receipt".to_string())
            })?;
        let current = store
            .read_text_versioned(crate::queue::control::CONTROL_BLOB)
            .await
            .map_err(|error| {
                DeployError(format!(
                    "cannot read queue before recorded restoration: {error}"
                ))
            })?;
        let intended = crate::queue::control::QueueControl {
            paused: false,
            reason: format!(
                "storage reconciliation {transaction} {}",
                if rollback { "rolled back" } else { "activated" }
            ),
            since: Utc::now().to_rfc3339(),
            by: "stado storage-root-reconcile".to_string(),
        };
        let owned_body = owned.intended.to_json();
        let superseding = if current
            .as_ref()
            .is_some_and(|versioned| versioned.content == owned_body)
        {
            None
        } else {
            Some(parse_queue_control(
                current.as_ref().map(|value| value.content.as_str()),
            )?)
        };
        fence.queue.restoration = Some(QueueEffect {
            status: if superseding.is_some() {
                "superseded"
            } else {
                "restore_intent"
            }
            .to_string(),
            expected_version: current.as_ref().map(|value| value.version.clone()),
            expected_content: current.as_ref().map(|value| value.content.clone()),
            intended,
            superseding,
        });
        write_fence(storage_target, transaction, fence, runner).await?;
    }
    let restoration = fence
        .queue
        .restoration
        .as_ref()
        .expect("queue restoration was initialized");
    match restoration.status.as_str() {
        "applied" => {
            fence.queue.resumed = true;
        }
        "superseded" => {
            fence.queue.resumed = false;
        }
        "restore_intent" => match execute_queue_effect(store, restoration).await? {
            QueueEffectOutcome::Applied => {
                fence
                    .queue
                    .restoration
                    .as_mut()
                    .expect("queue restoration was initialized")
                    .status = "applied".to_string();
                fence.queue.resumed = true;
                write_fence(storage_target, transaction, fence, runner).await?;
            }
            QueueEffectOutcome::Superseded(current) => {
                let restoration = fence
                    .queue
                    .restoration
                    .as_mut()
                    .expect("queue restoration was initialized");
                restoration.status = "superseded".to_string();
                restoration.superseding = Some(current);
                fence.queue.resumed = false;
                write_fence(storage_target, transaction, fence, runner).await?;
            }
        },
        status => {
            return Err(DeployError(format!(
                "queue restoration has invalid state {status:?}"
            )));
        }
    }
    Ok(())
}

async fn prove_listener_closed(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<(), DeployError> {
    let script = format!(
        r#"PORT={} /usr/bin/python3 - <<'PY'
import os, socket, time
port = int(os.environ['PORT'])
deadline = time.monotonic() + 30
while True:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.settimeout(0.2)
    result = probe.connect_ex(('127.0.0.1', port))
    probe.close()
    if result != 0:
        print('STADO_LISTENER_CLOSED\t' + str(port))
        break
    if time.monotonic() >= deadline:
        raise SystemExit('object API listener remained open')
    time.sleep(0.2)
PY"#,
        port
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_LISTENER_CLOSED\t{port}");
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(format!(
            "object API listener on {}:{port} did not close: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

fn qualified_copy_required(preflight: &Value) -> Result<bool, DeployError> {
    let backup = preflight
        .get("backup_qualified")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError("preflight omitted the backup qualified inventory".to_string())
        })?;
    let primary = preflight
        .get("primary_qualified")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError("preflight omitted the primary qualified inventory".to_string())
        })?;
    let primary_by_path = primary
        .iter()
        .filter_map(|item| {
            item.get("path")
                .and_then(Value::as_str)
                .map(|path| (path, item))
        })
        .collect::<BTreeMap<_, _>>();
    Ok(backup.iter().any(|item| {
        item.get("path")
            .and_then(Value::as_str)
            .and_then(|path| primary_by_path.get(path).copied())
            != Some(item)
    }))
}
fn physical_file_identity<'a>(
    preflight: &'a Value,
    inventory: &str,
    path: &str,
) -> Option<&'a Value> {
    preflight
        .get(inventory)?
        .get("files")?
        .as_array()?
        .iter()
        .find(|item| item.get("path").and_then(Value::as_str) == Some(path))
        .and_then(|item| item.get("body"))
}

async fn snapshot_unit_file(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<FileSnapshot>, DeployError> {
    let script = format!(
        r#"STADO_UNIT_PATH={} /usr/bin/python3 - <<'PY'
import base64, hashlib, json, os, stat
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))
try:
    info = os.lstat(path)
except FileNotFoundError:
    print('STADO_UNIT_SNAPSHOT\tabsent')
    raise SystemExit(0)
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
    raise SystemExit('unit path is not a regular non-symlink file')
with open(path, 'rb') as handle:
    body = handle.read()
print('STADO_UNIT_SNAPSHOT\t' + json.dumps({{
    'body_base64': base64.b64encode(body).decode('ascii'),
    'sha256': hashlib.sha256(body).hexdigest(),
    'mode': stat.S_IMODE(info.st_mode),
    'uid': info.st_uid,
    'gid': info.st_gid,
}}, sort_keys=True, separators=(',', ':')))
PY"#,
        shlex_quote(path)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "unit snapshot failed for {path} on {}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    let value = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNIT_SNAPSHOT\t"))
        .ok_or_else(|| DeployError("unit snapshot returned no marker".to_string()))?;
    if value == "absent" {
        return Ok(None);
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| DeployError(format!("unit snapshot is invalid: {error}")))
}
fn unit_declared_environment(
    candidate: &ServiceCandidate,
    snapshot: Option<&FileSnapshot>,
) -> Result<BTreeMap<String, String>, DeployError> {
    let Some(snapshot) = snapshot else {
        return Ok(BTreeMap::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&snapshot.body_base64)
        .map_err(|error| DeployError(format!("unit snapshot base64 is invalid: {error}")))?;
    let content = String::from_utf8(bytes)
        .map_err(|error| DeployError(format!("unit snapshot is not UTF-8: {error}")))?;
    let kind = if candidate.declared.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    let unit = service::UnitFile {
        host: candidate.target.name.clone(),
        unit: candidate.declared.unit_id().to_string(),
        path: candidate.declared.path.clone(),
        kind,
        content,
    };
    let parsed = service::unit_environment(&unit)?;
    Ok(parsed.env.into_iter().collect())
}

fn prepared_script(body: String) -> PreparedScript {
    PreparedScript {
        sha256: hex::encode(Sha256::digest(body.as_bytes())),
        body,
    }
}
async fn correlate_served_store(
    target: &crate::targets::ComputeTarget,
    port: u16,
    preflight: &Value,
    primary_after_commit: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let payload = serde_json::to_vec(&json!({
        "primary": preflight.get("primary_qualified"),
        "backup": preflight.get("backup_qualified"),
        "primary_physical": preflight.get("primary_physical"),
        "backup_physical": preflight.get("backup_physical"),
        "primary_after_commit": primary_after_commit,
    }))
    .map_err(|error| DeployError(format!("cannot encode served-store inventory: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let script = format!(
        r#"STADO_OBJECT_PORT={port} /usr/bin/python3 - <<'PY'
import base64, hashlib, json, os, urllib.parse, urllib.request
payload = json.loads(base64.b64decode('{encoded}'))
port = int(os.environ['STADO_OBJECT_PORT'])
token_path = os.path.expanduser('~/.stado/queue-object-api-token')
with open(token_path, encoding='utf-8') as handle:
    token = handle.read().strip()
if not token:
    raise SystemExit('object API correlation token is empty')
headers = {{'Authorization': 'Bearer ' + token}}
base = 'http://127.0.0.1:' + str(port)
request = urllib.request.Request(base + '/api/object/list?namespace=probierz&prefix=', headers=headers)
with urllib.request.urlopen(request, timeout=30) as response:
    listed = json.load(response)
keys = sorted(item.get('key') for item in listed.get('objects', []) if isinstance(item.get('key'), str))
def identities(name):
    result = {{}}
    prefix = 'ecosystem/probierz/'
    for item in payload[name]:
        path = item.get('path', '')
        if not path.startswith(prefix):
            continue
        result[path[len(prefix):]] = item.get('body')
    return result
primary_before = identities('primary')
backup = identities('backup')
primary = dict(primary_before)
if payload.get('primary_after_commit'):
    primary.update(backup)
served = {{}}
for key in keys:
    uri = 'stado://probierz/' + key
    url = base + '/api/object?uri=' + urllib.parse.quote(uri, safe='')
    request = urllib.request.Request(url, headers=headers)
    digest = hashlib.sha256()
    size = 0
    with urllib.request.urlopen(request, timeout=60) as response:
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
    served[key] = {{'sha256': digest.hexdigest(), 'bytes': size}}
matches_primary = keys == sorted(primary) and all(served[key] == primary[key] for key in keys)
matches_backup = keys == sorted(backup) and all(served[key] == backup[key] for key in keys)
if not matches_primary and not matches_backup:
    raise SystemExit('object API does not serve either complete physical qualified root')
authority = 'identical' if matches_primary and matches_backup else 'A' if matches_primary else 'B'
def physical_identity(name, path):
    for item in payload[name].get('files', []):
        if item.get('path') == path:
            return item.get('body')
    return None
object_mappings = [{{
    'backend': 'stado-object-api', 'namespace': 'probierz', 'key': key,
    'physical_path': 'ecosystem/probierz/' + key, 'identity': served[key],
}} for key in keys]
registry_mappings = [
    {{'root': 'A', 'backend': 'local', 'namespace': None, 'key': 'registry.json',
      'physical_path': 'registry.json',
      'identity': physical_identity('primary_physical', 'registry.json')}},
    {{'root': 'B', 'backend': 'local', 'namespace': None, 'key': 'registry.json',
      'physical_path': 'registry.json',
      'identity': physical_identity('backup_physical', 'registry.json')}},
    {{'root': 'served', 'backend': 'stado-object', 'namespace': None,
      'key': 'registry.json', 'physical_path': None,
      'observation': 'client namespace was not observable from the object API'}},
]
print('STADO_SERVED_STORE\t' + json.dumps({{
    'object_authority': authority,
    'endpoint': base,
    'object_store': {{'backend': 'stado-object-api', 'namespace': 'probierz',
                     'objects': object_mappings}},
    'registry_store': {{'mappings': registry_mappings}},
    'primary_root': os.path.expanduser('~/.stado/local-storage'),
    'backup_root': os.path.expanduser('~/.stado/local-backup'),
}}, sort_keys=True, separators=(',', ':')))
PY"#
    );
    let output = host_channel::run_script_with_timeout(target, &script, TIMEOUT, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "object API physical-store correlation failed on {}:{port}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_SERVED_STORE\t"))
        .ok_or_else(|| DeployError("object API correlation returned no evidence".to_string()))
        .and_then(|body| {
            serde_json::from_str(body)
                .map_err(|error| DeployError(format!("object API correlation is invalid: {error}")))
        })
}
async fn observe_object_runtime(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let script = format!(
        r#"python3 - <<'PY'
import json, urllib.request
with urllib.request.urlopen('http://127.0.0.1:{port}/api/state.json', timeout=30) as response:
    state = json.load(response)
print('STADO_STORAGE_RECONCILE\t' + json.dumps(state, sort_keys=True))
PY
"#
    );
    let output = host_channel::run_script_with_timeout(target, &script, TIMEOUT, runner).await?;
    parse_remote_payload(&output)
}

fn capture_storage_roots(
    transaction: &str,
    runtime: Value,
    writer: &WriterFence,
    staged: &super::host_release::StagedRelease,
) -> Result<StorageRoots, DeployError> {
    let directory = transaction_directory(transaction)?;
    let home = directory.ancestors().nth(3).ok_or_else(|| {
        DeployError("transaction directory has no Stado data directory".to_string())
    })?;
    let primary = home.join("local-storage").to_string_lossy().into_owned();
    let backup = home.join("local-backup").to_string_lossy().into_owned();
    let storage = runtime.get("storage").ok_or_else(|| {
        DeployError("object API state omitted its constructed storage handle".to_string())
    })?;
    let pid = storage.get("pid").and_then(Value::as_u64);
    if pid != writer.prior_pid.as_deref().and_then(|pid| pid.parse().ok())
        || writer.prior_sha256.as_deref() != Some(staged.staged_sha256.as_str())
    {
        return Err(DeployError(format!(
            "object API identity differs from the captured process or staged declared runtime: \
             API PID {pid:?}, captured PID {:?}, mapped SHA-256 {:?}, staged SHA-256 {}",
            writer.prior_pid, writer.prior_sha256, staged.staged_sha256,
        )));
    }
    if storage.get("backend").and_then(Value::as_str) != Some("local")
        || storage
            .pointer("/write_fence/protocol")
            .and_then(Value::as_str)
            != Some(crate::queue::LocalBackend::WRITE_FENCE_PROTOCOL)
    {
        return Err(DeployError(
            "object API does not report the local storage write-fence protocol; \
             the declared release must converge before a storage handoff"
                .to_string(),
        ));
    }
    let prior_primary = storage
        .get("local_path")
        .and_then(Value::as_str)
        .filter(|path| *path == primary || *path == backup)
        .ok_or_else(|| {
            DeployError(format!(
                "object API constructed root {:?} is outside fixed roots {primary:?} and {backup:?}",
                storage.get("local_path")
            ))
        })?
        .to_string();
    let prior_backup = match storage.get("backup").filter(|value| !value.is_null()) {
        None => None,
        Some(mirror) => {
            let path = mirror
                .get("local_path")
                .and_then(Value::as_str)
                .filter(|path| *path == primary || *path == backup);
            if mirror.get("backend").and_then(Value::as_str) != Some("local")
                || path.is_none()
                || path == Some(prior_primary.as_str())
            {
                return Err(DeployError(format!(
                    "object API constructed mirror is outside the distinct fixed A/B roots: {mirror}"
                )));
            }
            path.map(str::to_string)
        }
    };
    Ok(StorageRoots {
        primary,
        backup,
        prior_primary,
        prior_backup,
        runtime,
    })
}

async fn acquire_storage_write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    guard: &mut Option<std::fs::File>,
    runner: &Runner,
) -> Result<(), DeployError> {
    use crate::queue::LocalBackend;
    let roots = fence.roots.as_ref().ok_or_else(|| {
        DeployError("lifecycle fence omitted its observed storage roots".to_string())
    })?;
    let root = PathBuf::from(&roots.primary);
    let paths = LocalBackend::write_fence_paths(&root)
        .ok_or_else(|| DeployError("primary root has no storage write-fence path".to_string()))?;
    if LocalBackend::write_fence_paths(Path::new(&roots.backup)) != Some(paths.clone()) {
        return Err(DeployError(
            "A and B do not share the same storage write fence".to_string(),
        ));
    }
    if fence.write_fence.is_none() {
        fence.write_fence = Some(WriteFenceEffect {
            status: "acquire_intent".to_string(),
            intent: json!({
                "schema": LocalBackend::WRITE_FENCE_PROTOCOL,
                "transaction": transaction,
                "primary_root": roots.primary,
                "backup_root": roots.backup,
                "prepared_at": Utc::now().timestamp(),
            }),
            acquired_at: None,
            released_at: None,
        });
        write_fence(target, transaction, fence, runner).await?;
    }
    let effect = fence
        .write_fence
        .as_ref()
        .expect("write intent was recorded");
    if effect.status == "released" {
        return Ok(());
    }
    if guard.is_none() {
        let file = LocalBackend::open_write_fence_lock(&root)
            .map_err(|error| DeployError(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(DeployError(
                            "in-flight local storage writes did not finish within 30 seconds; \
                             the recorded handoff remains resumable"
                                .to_string(),
                        ));
                    }
                    sleep(Duration::from_millis(25)).await;
                }
                Err(error) => {
                    return Err(DeployError(format!(
                        "cannot acquire storage write fence {}: {error}",
                        paths.0.display()
                    )))
                }
            }
        }
        *guard = Some(file);
    }
    let state =
        LocalBackend::write_fence_state(&root).map_err(|error| DeployError(error.to_string()))?;
    match state.get("intent").filter(|value| !value.is_null()) {
        Some(intent) if intent == &effect.intent => {}
        Some(intent) => {
            return Err(DeployError(format!(
                "storage write fence belongs to a different recorded intent: {intent}"
            )))
        }
        None if effect.status == "acquire_intent" => {
            atomic_json_file(&paths.1, &effect.intent, "storage write-fence intent")?;
        }
        None if effect.status == "release_intent" => return Ok(()),
        None => {
            return Err(DeployError(
                "acquired storage write-fence intent disappeared; refusing to reconstruct it"
                    .to_string(),
            ))
        }
    }
    if fence.write_fence.as_ref().unwrap().status == "acquire_intent" {
        let effect = fence.write_fence.as_mut().unwrap();
        effect.status = "acquired".to_string();
        effect.acquired_at = Some(Utc::now().timestamp());
        write_fence(target, transaction, fence, runner).await?;
    }
    Ok(())
}

async fn release_storage_write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    guard: &mut Option<std::fs::File>,
    runner: &Runner,
) -> Result<(), DeployError> {
    use crate::queue::LocalBackend;
    if fence.write_fence.is_none() {
        return Ok(());
    }
    acquire_storage_write_fence(target, transaction, fence, guard, runner).await?;
    if fence.write_fence.as_ref().unwrap().status == "released" {
        return Ok(());
    }
    fence.write_fence.as_mut().unwrap().status = "release_intent".to_string();
    write_fence(target, transaction, fence, runner).await?;
    let root = Path::new(&fence.roots.as_ref().unwrap().primary);
    let (_, intent_path) = LocalBackend::write_fence_paths(root).unwrap();
    let state =
        LocalBackend::write_fence_state(root).map_err(|error| DeployError(error.to_string()))?;
    if let Some(intent) = state.get("intent").filter(|value| !value.is_null()) {
        if intent != &fence.write_fence.as_ref().unwrap().intent {
            return Err(DeployError(
                "storage write-fence intent changed before release".to_string(),
            ));
        }
        std::fs::remove_file(&intent_path).map_err(|error| {
            DeployError(format!("cannot release {}: {error}", intent_path.display()))
        })?;
        std::fs::File::open(intent_path.parent().unwrap())
            .and_then(|directory| directory.sync_all())
            .map_err(|error| DeployError(format!("cannot sync write-fence release: {error}")))?;
    }
    let effect = fence.write_fence.as_mut().unwrap();
    effect.status = "released".to_string();
    effect.released_at = Some(Utc::now().timestamp());
    write_fence(target, transaction, fence, runner).await?;
    *guard = None;
    Ok(())
}

async fn capture_fenced_preflight(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    if fence.preflight_evidence.is_some() {
        return Ok(());
    }
    let mut preflight = remote_phase(target, transaction, PREFLIGHT, runner).await?;
    let writer = fence
        .writers
        .iter()
        .find(|writer| writer.role == "object-api")
        .ok_or_else(|| DeployError("fence omitted its object API".to_string()))?;
    let correlation = correlate_served_store(
        target,
        writer
            .listener_port
            .ok_or_else(|| DeployError("object API port is absent".to_string()))?,
        &preflight,
        false,
        runner,
    )
    .await?;
    let authority = correlation
        .get("object_authority")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("fenced API proof omitted its authority".to_string()))?;
    let roots = fence.roots.as_ref().unwrap();
    if qualified_copy_required(&preflight)? && roots.prior_primary != roots.backup {
        return Err(DeployError(
            "object API's constructed authority is A and B differs; refusing a B-winning copy"
                .to_string(),
        ));
    }
    let prior_root = if roots.prior_primary == roots.primary {
        "A"
    } else {
        "B"
    };
    if !matches!(authority, "identical") && authority != prior_root {
        return Err(DeployError(
            "fenced API bytes disagree with its constructed storage root".to_string(),
        ));
    }
    let inventory = if prior_root == "A" {
        "primary_physical"
    } else {
        "backup_physical"
    };
    let configuration = json!({
        "object_api": {
            "runtime": roots.runtime,
            "observed_loaded_environment": writer.prior_loaded_environment,
            "unit_declaration": writer.unit_declared_environment,
            "registry_declaration": writer.registry_declared_environment,
        },
        "dashboard_registry_store": {
            "backend": "local", "namespace": Value::Null, "key": "registry.json",
            "physical_root": prior_root,
            "identity": physical_file_identity(&preflight, inventory, "registry.json"),
        },
    });
    let report = preflight
        .as_object_mut()
        .ok_or_else(|| DeployError("fenced preflight report is not an object".to_string()))?;
    report.insert("served_store".to_string(), correlation);
    report.insert("effective_configuration".to_string(), configuration);
    fence.preflight_evidence = Some(write_json_evidence(
        transaction,
        PREFLIGHT_EVIDENCE_FILE,
        &preflight,
        "fenced preflight evidence",
        true,
    )?);
    write_fence(target, transaction, fence, runner).await
}

async fn prepare_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
    write_guard: &mut Option<std::fs::File>,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = match read_fence(storage_target, transaction, runner).await? {
        Some(existing) => existing,
        None => {
            let resident_owner = resident_owner_retention(transaction)?;
            let resident_owner_unit = resident_owner
                .get("native_manager")
                .and_then(|manager| manager.get("service"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    DeployError("resident owner evidence omitted its exact service".to_string())
                })?
                .to_string();
            let services = registry_services(storage_target, &resident_owner_unit, runner).await?;
            let repository_runner_gate = repository_runner_gate().await?;
            let staged_runtime = super::host_release::stage_declared_release(
                &storage_target.name,
                "stado",
                storage_target
                    .managed_versions
                    .get("stado")
                    .ok_or_else(|| {
                        DeployError("target has no current declared Stado runtime".to_string())
                    })?,
                runner,
            )
            .await?;
            let current_runner = repository_runner_gate
                .as_ref()
                .and_then(|gate| gate.get("current_runner"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let store = crate::queue::JobStorage::new().await.map_err(|error| {
                DeployError(format!("cannot read queue before fencing: {error}"))
            })?;
            let prior_versioned = store
                .read_text_versioned(crate::queue::control::CONTROL_BLOB)
                .await
                .map_err(|error| DeployError(format!("cannot read prior queue state: {error}")))?;
            let prior = parse_queue_control(
                prior_versioned
                    .as_ref()
                    .map(|versioned| versioned.content.as_str()),
            )?;
            let mut writers = Vec::new();
            let mut transport_retained = Vec::new();
            let mut owning_runner_found = false;
            let mut non_storage_retained = Vec::new();
            let mut object_port = None;
            for candidate in &services {
                let observed_role =
                    service_role(candidate.declared.unit_id(), &candidate.observed_command);
                if observed_role == "other" && candidate.storage_evidence.is_empty() {
                    non_storage_retained.push(json!({
                        "target": candidate.target.name.clone(),
                        "label": candidate.declared.unit_id(),
                        "loaded_domains": candidate.loaded_domains.clone(),
                        "observed_command": candidate.observed_command.clone(),
                        "reason": "no Stado/runner/object-API role or local-storage route evidence",
                    }));
                    continue;
                }
                let state = super::service_label_print::print_label(
                    &candidate.target,
                    candidate.declared.unit_id(),
                    service::BootoutScope::Any,
                    runner,
                )
                .await?;
                let command = state.runs().unwrap_or(&candidate.observed_command);
                let mut role = service_role(candidate.declared.unit_id(), command).to_string();
                if role == "other" {
                    role = "writer".to_string();
                }
                if role == "runner"
                    && current_runner.as_deref().is_some_and(|current| {
                        current_runner_candidate(candidate, command, current)
                    })
                {
                    role = "current-runner".to_string();
                    owning_runner_found = true;
                }
                let autostart = service::label_autostart(
                    &candidate.target,
                    candidate.declared.unit_id(),
                    runner,
                )
                .await?;
                if role == "object-api" {
                    let backup_backend = state.loaded_environment.get("WC_BACKUP_STORAGE_BACKEND");
                    let backup_path = state.loaded_environment.get("WC_BACKUP_LOCAL_STORAGE_PATH");
                    if state.pid.is_none()
                        || state.process_started_at.is_none()
                        || state.process_executable.is_none()
                        || state.process_device.is_none()
                        || state.process_inode.is_none()
                        || state.process_sha256.is_none()
                    {
                        return Err(DeployError(format!(
                            "{} cannot be fenced without a mapped-inode image identity",
                            candidate.declared.unit_id()
                        )));
                    }
                    let loaded_routing_observed = state
                        .loaded_environment
                        .get("WC_STORAGE_BACKEND")
                        .map(String::as_str)
                        == Some("local")
                        && state
                            .loaded_environment
                            .get("WC_LOCAL_STORAGE_PATH")
                            .is_some_and(|path| !path.is_empty())
                        && state
                            .loaded_environment
                            .get("STADO_CONFIG")
                            .is_some_and(|path| !path.is_empty())
                        && backup_backend.is_some() == backup_path.is_some();
                    if state.loaded_environment.contains_key("WC_STORAGE_BACKEND")
                        && !loaded_routing_observed
                    {
                        return Err(DeployError(format!(
                            "{} reported an incomplete loaded storage route",
                            candidate.declared.unit_id()
                        )));
                    }
                    object_port = command_u16_option(command, "--port");
                }
                if matches!(role.as_str(), "transport" | "current-runner") {
                    if role == "current-runner" && state.pid.is_none() {
                        return Err(DeployError(
                            "Actions runner gate did not map its owning live native process"
                                .to_string(),
                        ));
                    }
                    if state.pid.is_some()
                        && (state.process_started_at.is_none()
                            || state.process_device.is_none()
                            || state.process_inode.is_none()
                            || state.process_sha256.is_none())
                    {
                        return Err(DeployError(format!(
                            "retained transport {} has no mapped-inode image identity",
                            candidate.declared.unit_id()
                        )));
                    }
                    if state.loaded() || state.pid.is_some() {
                        transport_retained.push(json!({
                            "host": candidate.target.name.clone(),
                            "label": candidate.declared.unit_id(),
                            "loaded_domains": candidate.loaded_domains.clone(),
                            "autostart": autostart,
                            "state": state.to_json(),
                        }));
                    }
                    continue;
                }
                let was_loaded = state.loaded() || !candidate.loaded_domains.is_empty();
                let was_runnable = state.pid.is_some();
                let canonical_stado_recovery = state
                    .process_executable
                    .as_deref()
                    .is_some_and(|path| path.ends_with("/.stado/bin/stado"))
                    && staged_runtime.staged_sha256.len() == 64;
                if was_runnable
                    && (state.process_started_at.is_none()
                        || state.process_executable.is_none()
                        || state.process_device.is_none()
                        || state.process_inode.is_none()
                        || (state.process_sha256.is_none() && !canonical_stado_recovery))
                {
                    return Err(DeployError(format!(
                        "{} cannot be fenced without a mapped-inode process identity or its \
                         digest-verified canonical restoration plan",
                        candidate.declared.unit_id()
                    )));
                }
                if (was_loaded || was_runnable) && candidate.declared.path.is_empty() {
                    return Err(DeployError(format!(
                        "{} has no unit path from which its exact prior lifecycle can be restored",
                        candidate.declared.unit_id()
                    )));
                }
                let listener_port = (role == "object-api").then_some(object_port).flatten();
                if role == "object-api" && listener_port.is_none() {
                    return Err(DeployError(
                        "object API listener port is absent from its loaded argv".to_string(),
                    ));
                }
                let pending = was_loaded
                    || was_runnable
                    || autostart.values().copied().any(|enabled| enabled);
                let unit_snapshot =
                    snapshot_unit_file(&candidate.target, &candidate.declared.path, runner).await?;
                if pending && unit_snapshot.is_none() {
                    return Err(DeployError(format!(
                        "{} has no exact unit bytes for restoration",
                        candidate.declared.unit_id()
                    )));
                }
                let unit_declared_environment =
                    unit_declared_environment(candidate, unit_snapshot.as_ref())?;
                writers.push(WriterFence {
                    target: candidate.target.name.clone(),
                    label: candidate.declared.unit_id().to_string(),
                    role,
                    storage_evidence: candidate.storage_evidence.iter().cloned().collect(),
                    path: candidate.declared.path.clone(),
                    listener_port,
                    was_loaded,
                    was_runnable,
                    loaded_domains: candidate.loaded_domains.clone(),
                    autostart,
                    prior_pid: state.pid,
                    prior_started_at: state.process_started_at,
                    prior_loaded_environment: state.loaded_environment,
                    registry_declared_environment: candidate.declared.env.clone(),
                    unit_declared_environment,
                    prior_executable: state.process_executable,
                    prior_sha256: state.process_sha256,
                    prior_device: state.process_device,
                    prior_inode: state.process_inode,
                    unit_snapshot,
                    prior_native_state: state.state,
                    prior_last_exit_code: state.last_exit_code,
                    prior_restart: state.restart,
                    prior_triggers: state.triggers,
                    forward_object_recovery: None,
                    rollback_object_recovery: None,
                    status: if pending { "pending" } else { "stopped" }.to_string(),
                    restored_pid: None,
                    restored_started_at: None,
                    restored_loaded_environment: BTreeMap::new(),
                    restored_executable: None,
                    restored_sha256: None,
                    restored_device: None,
                    restored_inode: None,
                    restored_route: None,
                });
            }
            writers.sort_by_key(|writer| stop_priority(&writer.role));
            if !writers.iter().any(|writer| writer.role == "object-api") {
                return Err(DeployError(
                    "fleet service inventory did not resolve the canonical object API".to_string(),
                ));
            }
            if repository_runner_gate.is_some() && !owning_runner_found {
                return Err(DeployError(
                    "runner gate did not map its owning native runner service".to_string(),
                ));
            }
            let runtime = observe_object_runtime(
                storage_target,
                object_port.ok_or_else(|| DeployError("object API port is absent".to_string()))?,
                runner,
            )
            .await?;
            let object_writer = writers
                .iter_mut()
                .find(|writer| writer.role == "object-api")
                .expect("canonical object API writer was required above");
            let roots =
                capture_storage_roots(transaction, runtime, object_writer, &staged_runtime)?;
            object_writer.forward_object_recovery = Some(object_recovery_script(
                object_writer,
                &roots.primary,
                Some(&roots.backup),
            )?);
            object_writer.rollback_object_recovery = Some(object_recovery_script(
                object_writer,
                &roots.prior_primary,
                roots.prior_backup.as_deref(),
            )?);
            let initial = LifecycleFence {
                schema: FENCE_SCHEMA.to_string(),
                transaction: transaction.to_string(),
                status: "preparing".to_string(),
                queue: QueueFence {
                    was_paused: prior.paused,
                    drained: false,
                    resumed: false,
                    pause: (!prior.paused).then(|| QueueEffect {
                        status: "pause_intent".to_string(),
                        expected_version: prior_versioned
                            .as_ref()
                            .map(|versioned| versioned.version.clone()),
                        expected_content: prior_versioned
                            .as_ref()
                            .map(|versioned| versioned.content.clone()),
                        intended: crate::queue::control::QueueControl {
                            paused: true,
                            reason: format!("storage reconciliation {transaction}"),
                            since: Utc::now().to_rfc3339(),
                            by: "stado storage-root-reconcile".to_string(),
                        },
                        superseding: None,
                    }),
                    restoration: None,
                },
                resident_owner,
                writers,
                transport_retained,
                non_storage_retained,
                staged_runtime: Some(staged_runtime),
                roots: Some(roots),
                write_fence: None,
                preflight_evidence: None,
                rollback_preparation: false,
                lease_acquisitions: Vec::new(),
                repository_runner_gate,
                prepared_at: Utc::now().timestamp(),
                rechecked_at: 0,
                activated_at: None,
                activation_sha256: None,
                restored_at: None,
            };
            write_fence(storage_target, transaction, &initial, runner).await?;
            initial
        }
    };
    if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
        return Err(DeployError(
            "durable lifecycle fence belongs to another transaction".to_string(),
        ));
    }
    refresh_resident_owner(storage_target, transaction, &mut fence, runner).await?;
    if fence.status == "fenced" {
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
        return recheck_lifecycle_fence(storage_target, transaction, runner).await;
    }
    if fence.status != "preparing" {
        return Err(DeployError(format!(
            "lifecycle fence cannot prepare from {}",
            fence.status
        )));
    }

    let store = if fence.write_fence.is_some() {
        if !fence.queue.drained
            || fence
                .lease_acquisitions
                .iter()
                .any(|entry| entry.lease.is_none())
        {
            return Err(DeployError(
                "storage write fence preceded queue draining or lease acquisition".to_string(),
            ));
        }
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
        None
    } else {
        Some(
            crate::queue::JobStorage::new()
                .await
                .map_err(|error| DeployError(format!("cannot open queue for fencing: {error}")))?,
        )
    };
    if let Some(store) = &store {
        const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
        let subjects = fence
            .writers
            .iter()
            .map(|writer| format!("service:{}:{}", writer.target, writer.label))
            .collect::<Vec<_>>();
        for subject in subjects {
            let index = match fence
                .lease_acquisitions
                .iter()
                .position(|entry| entry.subject_id == subject)
            {
                Some(index) => index,
                None => {
                    fence.lease_acquisitions.push(LeaseAcquisition {
                        subject_id: subject.clone(),
                        status: "acquire_intent".to_string(),
                        lease: None,
                        released_lease: None,
                    });
                    write_fence(storage_target, transaction, &fence, runner).await?;
                    fence.lease_acquisitions.len() - 1
                }
            };
            if fence.lease_acquisitions[index].lease.is_none() {
                let lease = crate::autonomy::storage::acquire_placement_lease(
                    store,
                    &subject,
                    transaction,
                    "stado storage-root-reconcile",
                    LEASE_TTL_SECONDS,
                    Utc::now(),
                )
                .await
                .map_err(|error| DeployError(format!("cannot acquire {subject}: {error}")))?
                .ok_or_else(|| DeployError(format!("active placement lease blocks {subject}")))?;
                fence.lease_acquisitions[index].lease = Some(lease);
                fence.lease_acquisitions[index].status = "acquired".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
        }
        renew_fence_leases(store, &mut fence).await?;
        write_fence(storage_target, transaction, &fence, runner).await?;

        if !fence.queue.drained {
            if let Some(pause) = fence.queue.pause.as_ref() {
                if pause.status != "applied" {
                    if pause.status != "pause_intent" {
                        return Err(DeployError(format!(
                            "queue pause has invalid state {:?}",
                            pause.status
                        )));
                    }
                    match execute_queue_effect(store, pause).await? {
                        QueueEffectOutcome::Applied => {
                            fence
                                .queue
                                .pause
                                .as_mut()
                                .expect("queue pause was initialized")
                                .status = "applied".to_string();
                            write_fence(storage_target, transaction, &fence, runner).await?;
                        }
                        QueueEffectOutcome::Superseded(current) => {
                            fence
                                .queue
                                .pause
                                .as_mut()
                                .expect("queue pause was initialized")
                                .superseding = Some(current);
                            return Err(DeployError(
                                "queue control changed after the exact pause intent was recorded"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
            let current = crate::queue::control::read(store)
                .await
                .map_err(|error| DeployError(format!("cannot recheck queue fence: {error}")))?;
            if !current.paused {
                return Err(DeployError(
                    "queue is not paused after its durable fencing transition".to_string(),
                ));
            }
            let deadline = Instant::now()
                + Duration::from_secs(crate::queue::control::default_drain_timeout_s());
            while !crate::queue::control::is_drained(store)
                .await
                .map_err(|error| DeployError(format!("cannot prove queue drained: {error}")))?
            {
                if Instant::now() >= deadline {
                    return Err(DeployError(
                        "queue remained active until the canonical drain deadline; fence retained"
                            .to_string(),
                    ));
                }
                sleep(Duration::from_secs(5)).await;
            }
            fence.queue.drained = true;
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
    }
    for index in 0..fence.writers.len() {
        let writer = &fence.writers[index];
        let current = super::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let current_autostart =
            service::label_autostart(storage_target, &writer.label, runner).await?;
        let process_matches_prior = current.loaded() == writer.was_loaded
            && current.pid.as_deref() == writer.prior_pid.as_deref()
            && current.process_started_at.as_deref() == writer.prior_started_at.as_deref()
            && current.process_executable.as_deref() == writer.prior_executable.as_deref()
            && current.process_device == writer.prior_device
            && current.process_inode == writer.prior_inode
            && match writer.prior_sha256.as_deref() {
                Some(expected) => current.process_sha256.as_deref() == Some(expected),
                None => writer
                    .prior_executable
                    .as_deref()
                    .is_some_and(|path| path.ends_with("/.stado/bin/stado")),
            }
            && current.loaded_environment == writer.prior_loaded_environment;
        match writer.status.as_str() {
            "pending" if process_matches_prior && current_autostart == writer.autostart => {}
            "stop_intent" if !current.loaded() && current.pid.is_none() => {
                if writer.autostart.iter().any(|(scope, enabled)| {
                    *enabled && current_autostart.get(scope) != Some(&false)
                }) {
                    return Err(DeployError(format!(
                        "{} stopped after an interrupted fence but remained enabled",
                        writer.label
                    )));
                }
                fence.writers[index].status = "stopped".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
            "stop_intent"
                if process_matches_prior
                    && writer.autostart.iter().all(|(scope, prior)| {
                        current_autostart
                            .get(scope)
                            .is_some_and(|current| current == prior || (*prior && !*current))
                    }) => {}
            "stopped" if !current.loaded() && current.pid.is_none() => {}
            state => {
                return Err(DeployError(format!(
                    "{} native state does not match resumable fence state {state:?}",
                    writer.label
                )));
            }
        }
    }

    for index in 0..fence.writers.len() {
        if fence.writers[index].status == "stopped" {
            continue;
        }
        if fence.writers[index].role == "object-api" {
            acquire_storage_write_fence(
                storage_target,
                transaction,
                &mut fence,
                write_guard,
                runner,
            )
            .await?;
            capture_fenced_preflight(storage_target, transaction, &mut fence, runner).await?;
        }
        if fence.writers[index].status == "pending" {
            fence.writers[index].status = "stop_intent".to_string();
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
        if let Some(store) = &store {
            renew_fence_leases(store, &mut fence).await?;
        }
        write_fence(storage_target, transaction, &fence, runner).await?;
        let label = fence.writers[index].label.clone();
        for (scope, enabled) in fence.writers[index].autostart.clone() {
            if enabled {
                service::set_label_autostart(storage_target, &label, &scope, false, runner).await?;
            }
        }
        let disabled = service::label_autostart(storage_target, &label, runner).await?;
        if fence.writers[index]
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && disabled.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "{label} remained enabled after persistent lifecycle disable"
            )));
        }
        if fence.writers[index].was_loaded || fence.writers[index].was_runnable {
            let (state, detail) =
                service::bootout_label(storage_target, &label, service::BootoutScope::Any, runner)
                    .await?;
            if !matches!(state.as_str(), "booted_out" | "absent") {
                return Err(DeployError(format!("{label} did not boot out: {detail}")));
            }
        }
        let state = super::service_label_print::print_label(
            storage_target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "{label} remained loaded after writer fencing"
            )));
        }
        if let Some(port) = fence.writers[index].listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
        fence.writers[index].status = "stopped".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }
    fence.status = "fenced".to_string();
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

async fn recheck_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    if fence.status != "fenced"
        || !fence.queue.drained
        || fence
            .writers
            .iter()
            .any(|writer| writer.status != "stopped")
    {
        return Err(DeployError(
            "durable lifecycle fence is not in the fenced/drained state".to_string(),
        ));
    }
    fence.resident_owner = resident_owner_retention(transaction)?;
    for writer in &fence.writers {
        let state = super::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "writer {} on {} resumed during the storage fence",
                writer.label, writer.target
            )));
        }
        let autostart = service::label_autostart(storage_target, &writer.label, runner).await?;
        if writer
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && autostart.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "writer {} became enabled during the storage fence",
                writer.label
            )));
        }
        if let Some(port) = writer.listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
    }
    for retained in &fence.transport_retained {
        let label = retained
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let state = super::service_label_print::print_label(
            storage_target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if !state.loaded()
            || state.pid.is_none()
            || state.process_started_at.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none()
            || state.process_sha256.is_none()
        {
            return Err(DeployError(format!(
                "retained transport {label} is no longer a runnable mapped image"
            )));
        }
        let prior = retained
            .get("state")
            .and_then(Value::as_object)
            .ok_or_else(|| DeployError(format!("retained transport {label} has no prior state")))?;
        let current = state.to_json();
        for field in [
            "pid",
            "process_started_at",
            "process_executable",
            "process_device",
            "process_inode",
            "process_sha256",
        ] {
            if current.get(field) != prior.get(field) {
                return Err(DeployError(format!(
                    "retained transport {label} changed mapped identity field {field}"
                )));
            }
        }
        let autostart = service::label_autostart(storage_target, label, runner).await?;
        if retained.get("autostart") != Some(&json!(autostart)) {
            return Err(DeployError(format!(
                "retained transport {label} changed native autostart state"
            )));
        }
    }
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

fn restore_priority(role: &str) -> u8 {
    match role {
        "object-api" => 0,
        "coordinator" => 1,
        "agent" | "disk-cleanup" => 2,
        "release-agent" => 3,
        "runner" => 4,
        "current-runner" => 5,
        _ => 2,
    }
}

fn managed_writer(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
) -> service::ManagedService {
    let kind = if writer.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    managed_from_unit(target, &writer.label, &writer.path, kind)
}

fn object_recovery_script(
    writer: &WriterFence,
    primary: &str,
    backup: Option<&str>,
) -> Result<PreparedScript, DeployError> {
    if primary.is_empty() || backup == Some("") {
        return Err(DeployError(
            "prepared object recovery contains an empty physical root".to_string(),
        ));
    }
    let config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            DeployError(
                "object API has neither observed nor declared STADO_CONFIG for recovery"
                    .to_string(),
            )
        })?;
    let port = writer
        .listener_port
        .ok_or_else(|| DeployError("captured object API port is absent".to_string()))?;
    let body = ROLLBACK_OBJECT_API_SCRIPT
        .replace("@PRIMARY@", &shlex_quote(primary))
        .replace(
            "@BACKUP_BACKEND@",
            &shlex_quote(if backup.is_some() { "local" } else { "" }),
        )
        .replace("@BACKUP@", &shlex_quote(backup.unwrap_or("")))
        .replace("@CONFIG@", &shlex_quote(config))
        .replace("@PORT@", &port.to_string());
    Ok(prepared_script(body))
}

fn validate_prepared_fence(fence: &LifecycleFence) -> Result<(), DeployError> {
    if fence.roots.is_none() {
        return Err(DeployError(
            "lifecycle fence omitted its constructed storage roots".to_string(),
        ));
    }
    if fence.status != "preparing"
        && !fence.rollback_preparation
        && fence.preflight_evidence.is_none()
    {
        return Err(DeployError(
            "lifecycle fence omitted its frozen preflight evidence".to_string(),
        ));
    }
    let staged_runtime_digest = fence
        .staged_runtime
        .as_ref()
        .map(|release| release.staged_sha256.as_str())
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    for writer in &fence.writers {
        if let Some(snapshot) = &writer.unit_snapshot {
            let body = base64::engine::general_purpose::STANDARD
                .decode(&snapshot.body_base64)
                .map_err(|error| {
                    DeployError(format!(
                        "{} unit snapshot is invalid: {error}",
                        writer.label
                    ))
                })?;
            if hex::encode(Sha256::digest(&body)) != snapshot.sha256 {
                return Err(DeployError(format!(
                    "{} unit snapshot digest does not match its exact bytes",
                    writer.label
                )));
            }
        }
        if writer.was_runnable
            && (writer.prior_pid.is_none()
                || writer.prior_started_at.is_none()
                || writer.prior_executable.is_none()
                || writer.prior_device.is_none()
                || writer.prior_inode.is_none()
                || (writer.prior_sha256.is_none()
                    && (staged_runtime_digest.is_none()
                        || !writer
                            .prior_executable
                            .as_deref()
                            .is_some_and(|path| path.ends_with("/.stado/bin/stado")))))
        {
            return Err(DeployError(format!(
                "{} has no complete mapped-inode process identity or digest-verified canonical \
                 restoration",
                writer.label
            )));
        }
        for script in [
            writer.forward_object_recovery.as_ref(),
            writer.rollback_object_recovery.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if hex::encode(Sha256::digest(script.body.as_bytes())) != script.sha256 {
                return Err(DeployError(format!(
                    "{} prepared recovery script digest changed",
                    writer.label
                )));
            }
        }
        if writer.role == "object-api"
            && (writer.forward_object_recovery.is_none()
                || writer.rollback_object_recovery.is_none())
        {
            return Err(DeployError(
                "object API has no immutable forward and captured-prior rollback configurations"
                    .to_string(),
            ));
        }
    }
    Ok(())
}
fn recovered_object_store(fence: &LifecycleFence) -> Result<crate::queue::JobStorage, DeployError> {
    let endpoint = fence
        .writers
        .iter()
        .find(|writer| writer.role == "object-api")
        .and_then(|writer| writer.restored_route.as_ref())
        .and_then(|proof| proof.get("served_store"))
        .and_then(|proof| proof.get("endpoint"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeployError("recovered object API proof omitted its endpoint".to_string())
        })?;
    let backend = crate::queue::StadoObjectBackend::new(
        endpoint,
        "probierz",
        "~/.stado/queue-object-api-token",
        "",
    )
    .map_err(|error| {
        DeployError(format!(
            "cannot bind typed observation to recovered object API: {error}"
        ))
    })?;
    Ok(crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "recovered-stado-object",
    ))
}

async fn restore_unit_snapshot(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let snapshot = writer
        .unit_snapshot
        .as_ref()
        .ok_or_else(|| DeployError(format!("{} has no captured exact unit bytes", writer.label)))?;
    let script = format!(
        r#"STADO_UNIT_PATH={} STADO_UNIT_BODY={} STADO_UNIT_SHA={} STADO_UNIT_MODE={} STADO_UNIT_UID={} STADO_UNIT_GID={} /usr/bin/python3 - <<'PY'
import base64, hashlib, os, stat, subprocess, tempfile
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))
body = base64.b64decode(os.environ['STADO_UNIT_BODY'])
expected = os.environ['STADO_UNIT_SHA']
if hashlib.sha256(body).hexdigest() != expected:
    raise SystemExit('captured unit bytes fail their digest')
expected_metadata = (int(os.environ['STADO_UNIT_MODE']),
                     int(os.environ['STADO_UNIT_UID']),
                     int(os.environ['STADO_UNIT_GID']))
work = os.path.expanduser('~/.stado/work/storage-root-reconcile-units')
os.makedirs(work, mode=0o700, exist_ok=True)
fd, temporary = tempfile.mkstemp(prefix='unit.', dir=work)
try:
    with os.fdopen(fd, 'wb') as handle:
        handle.write(body)
        handle.flush()
        os.fsync(handle.fileno())
    command = ['/usr/bin/sudo', '-n', '/usr/bin/install',
               '-m', format(expected_metadata[0], 'o'),
               '-o', os.environ['STADO_UNIT_UID'],
               '-g', os.environ['STADO_UNIT_GID'], temporary, path]
    result = subprocess.run(command, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, close_fds=False)
    if result.returncode != 0:
        raise SystemExit((result.stderr or result.stdout).strip())
finally:
    try:
        os.unlink(temporary)
    except FileNotFoundError:
        pass
info = os.lstat(path)
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
    raise SystemExit('restored unit is not a regular file')
observed_metadata = (stat.S_IMODE(info.st_mode), info.st_uid, info.st_gid)
if observed_metadata != expected_metadata:
    raise SystemExit('restored unit mode/uid/gid mismatch: expected ' +
                     str(expected_metadata) + ', observed ' + str(observed_metadata))
with open(path, 'rb') as handle:
    if hashlib.sha256(handle.read()).hexdigest() != expected:
        raise SystemExit('restored unit digest mismatch')
print('STADO_UNIT_RESTORED\t' + expected)
PY"#,
        shlex_quote(&writer.path),
        shlex_quote(&snapshot.body_base64),
        shlex_quote(&snapshot.sha256),
        snapshot.mode,
        snapshot.uid,
        snapshot.gid,
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_UNIT_RESTORED\t{}", snapshot.sha256);
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(format!(
            "exact unit restoration failed for {} on {}: {}",
            writer.label,
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

fn restored_state_matches(
    writer: &WriterFence,
    state: &super::service_label_print::LabelState,
    autostart: &BTreeMap<String, bool>,
    active_sha256: &str,
    roots: &StorageRoots,
    rollback: bool,
) -> bool {
    if autostart != &writer.autostart {
        return false;
    }
    let should_be_loaded = writer.was_loaded || writer.was_runnable;
    if state.loaded() != should_be_loaded {
        return false;
    }
    if !should_be_loaded {
        return state.pid.is_none();
    }
    if let Some(pid) = state.pid.as_deref() {
        if pid == "0"
            || state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none_or(|inode| inode == 0)
        {
            return false;
        }
        let expected_sha256 = if writer.role == "object-api"
            || writer
                .prior_executable
                .as_deref()
                .is_some_and(|path| executable_name(path) == "stado")
        {
            Some(active_sha256)
        } else {
            writer.prior_sha256.as_deref()
        };
        if state.process_sha256.as_deref() != expected_sha256 {
            return false;
        }
    } else if writer.role == "object-api"
        || (state.state.is_none()
            && state.last_exit_code.is_none()
            && state.restart.is_none()
            && state.triggers.is_none())
    {
        return false;
    }
    if writer.role != "object-api" {
        return true;
    }
    let loaded = &state.loaded_environment;
    if !loaded.contains_key("WC_STORAGE_BACKEND") {
        return true;
    }
    let expected_config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .map(String::as_str);
    if loaded.get("WC_STORAGE_BACKEND").map(String::as_str) != Some("local")
        || loaded.get("STADO_CONFIG").map(String::as_str) != expected_config
    {
        return false;
    }
    let (primary, backup) = if rollback {
        (roots.prior_primary.as_str(), roots.prior_backup.as_deref())
    } else {
        (roots.primary.as_str(), Some(roots.backup.as_str()))
    };
    loaded.get("WC_LOCAL_STORAGE_PATH").map(String::as_str) == Some(primary)
        && loaded
            .get("WC_BACKUP_STORAGE_BACKEND")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup.map(|_| "local")
        && loaded
            .get("WC_BACKUP_LOCAL_STORAGE_PATH")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup
}

fn durable_restored_state_matches(
    writer: &WriterFence,
    state: &super::service_label_print::LabelState,
) -> bool {
    (writer.role != "object-api" || writer.restored_route.is_some())
        && state.pid == writer.restored_pid
        && state.process_started_at == writer.restored_started_at
        && state.loaded_environment == writer.restored_loaded_environment
        && state.process_executable == writer.restored_executable
        && state.process_sha256 == writer.restored_sha256
        && state.process_device == writer.restored_device
        && state.process_inode == writer.restored_inode
}
async fn activate_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
    rollback: bool,
    write_guard: &mut Option<std::fs::File>,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    validate_prepared_fence(&fence)?;
    refresh_resident_owner(storage_target, transaction, &mut fence, runner).await?;
    let preflight = if fence.rollback_preparation {
        None
    } else {
        Some(read_json_evidence(
            transaction,
            PREFLIGHT_EVIDENCE_FILE,
            fence.preflight_evidence.as_ref().ok_or_else(|| {
                DeployError("lifecycle fence omitted frozen preflight evidence".to_string())
            })?,
            "preflight evidence",
        )?)
    };
    let roots = fence.roots.clone().ok_or_else(|| {
        DeployError("lifecycle fence omitted its observed storage roots".to_string())
    })?;
    let final_status = if rollback { "rolled_back" } else { "activated" };
    let admissible = if rollback {
        matches!(
            fence.status.as_str(),
            "preparing" | "fenced" | "rolling_back" | "restoring" | "rolled_back"
        )
    } else {
        matches!(
            fence.status.as_str(),
            "fenced" | "activating" | "restoring" | "activated"
        )
    };
    if !admissible {
        return Err(DeployError(format!(
            "lifecycle fence cannot {} from {}",
            if rollback { "roll back" } else { "activate" },
            fence.status
        )));
    }
    if fence.status == final_status {
        return Ok(fence);
    }
    if !matches!(fence.status.as_str(), "activating" | "restoring") {
        fence.status = if rollback {
            "rolling_back"
        } else {
            "activating"
        }
        .to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }

    if fence.write_fence.is_some() {
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
    }
    let staged_runtime = fence
        .staged_runtime
        .clone()
        .ok_or_else(|| DeployError("lifecycle fence has no staged declared runtime".to_string()))?;
    let active_sha256 =
        super::host_release::activate_staged_program(storage_target, &staged_runtime, runner)
            .await?;
    if fence
        .activation_sha256
        .as_deref()
        .is_some_and(|digest| digest != active_sha256)
    {
        return Err(DeployError(
            "persisted activation digest differs from the adopted active runtime".to_string(),
        ));
    }
    fence.activation_sha256 = Some(active_sha256.clone());
    fence
        .activated_at
        .get_or_insert_with(|| Utc::now().timestamp());
    fence.status = "restoring".to_string();
    write_fence(storage_target, transaction, &fence, runner).await?;

    let mut order = (0..fence.writers.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| restore_priority(&fence.writers[*index].role));
    let mut restored_store = None;
    for index in order {
        let label = fence.writers[index].label.clone();
        let mut state = super::service_label_print::print_label(
            storage_target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let mut autostart = service::label_autostart(storage_target, &label, runner).await?;
        let unit_matches = if fence.writers[index].role == "object-api" {
            true
        } else {
            snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
                == fence.writers[index].unit_snapshot
        };
        let was_durably_restored = fence.writers[index].status == "restored";
        let adopted = unit_matches
            && restored_state_matches(
                &fence.writers[index],
                &state,
                &autostart,
                &active_sha256,
                &roots,
                rollback,
            )
            && (!was_durably_restored
                || durable_restored_state_matches(&fence.writers[index], &state));
        if was_durably_restored && !adopted {
            return Err(DeployError(format!(
                "{label} drifted after its durable restored result"
            )));
        }
        if !adopted {
            if fence.writers[index].status != "restore_intent" {
                fence.writers[index].status = "restore_intent".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
            if fence.writers[index].role != "object-api"
                && fence.writers[index].unit_snapshot.is_some()
            {
                restore_unit_snapshot(storage_target, &fence.writers[index], runner).await?;
            }
            let requires_load =
                fence.writers[index].was_loaded || fence.writers[index].was_runnable;
            if requires_load {
                if fence.writers[index].role == "object-api" {
                    let prepared = if rollback {
                        fence.writers[index].rollback_object_recovery.as_ref()
                    } else {
                        fence.writers[index].forward_object_recovery.as_ref()
                    }
                    .ok_or_else(|| {
                        DeployError(format!("{label} has no prepared recovery configuration"))
                    })?;
                    let recovered = host_channel::run_script_with_timeout(
                        storage_target,
                        &prepared.body,
                        Duration::from_secs(240),
                        runner,
                    )
                    .await?;
                    if !recovered.ok() {
                        return Err(DeployError(format!(
                            "{label} did not restore through its prepared configuration: {}",
                            host_channel::last_error_line(&recovered, "remote command failed")
                        )));
                    }
                } else {
                    let writer = &fence.writers[index];
                    if !writer.autostart.values().copied().any(|enabled| enabled) {
                        let scope = writer
                            .loaded_domains
                            .first()
                            .map(String::as_str)
                            .or_else(|| writer.autostart.keys().next().map(String::as_str))
                            .ok_or_else(|| {
                                DeployError(format!(
                                    "{label} has no captured init-system scope for restoration"
                                ))
                            })?;
                        service::set_label_autostart(storage_target, &label, scope, true, runner)
                            .await?;
                    }
                    let declared = managed_writer(storage_target, writer);
                    let restarted =
                        service::restart_service(storage_target, &declared, runner).await?;
                    if !restarted.succeeded("restarted") {
                        return Err(DeployError(format!(
                            "{label} did not restore: {}",
                            restarted.failure()
                        )));
                    }
                }
            }
            for (scope, enabled) in fence.writers[index].autostart.clone() {
                service::set_label_autostart(storage_target, &label, &scope, enabled, runner)
                    .await?;
            }
            state = super::service_label_print::print_label(
                storage_target,
                &label,
                service::BootoutScope::Any,
                runner,
            )
            .await?;
            autostart = service::label_autostart(storage_target, &label, runner).await?;
            if !restored_state_matches(
                &fence.writers[index],
                &state,
                &autostart,
                &active_sha256,
                &roots,
                rollback,
            ) {
                return Err(DeployError(format!(
                    "{label} does not match its captured lifecycle and prepared runtime"
                )));
            }
            if fence.writers[index].role != "object-api"
                && snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
                    != fence.writers[index].unit_snapshot
            {
                return Err(DeployError(format!(
                    "{label} unit definition differs from its captured exact bytes"
                )));
            }
        }
        let restored_route = if fence.writers[index].role == "object-api" && was_durably_restored {
            Some(fence.writers[index].restored_route.clone().ok_or_else(|| {
                DeployError("durable object API result omitted its route proof".to_string())
            })?)
        } else if fence.writers[index].role == "object-api" {
            let port = fence.writers[index].listener_port.ok_or_else(|| {
                DeployError("object API listener port is absent from its fence".to_string())
            })?;
            let runtime = observe_object_runtime(storage_target, port, runner).await?;
            let storage = runtime.get("storage").ok_or_else(|| {
                DeployError("restored object API omitted its constructed storage".to_string())
            })?;
            let (expected_root, expected_backup) = if rollback {
                (roots.prior_primary.as_str(), roots.prior_backup.as_deref())
            } else {
                (roots.primary.as_str(), Some(roots.backup.as_str()))
            };
            let mirror_matches = match expected_backup {
                Some(path) => {
                    storage.pointer("/backup/backend").and_then(Value::as_str) == Some("local")
                        && storage
                            .pointer("/backup/local_path")
                            .and_then(Value::as_str)
                            == Some(path)
                }
                None => storage.get("backup").is_none_or(Value::is_null),
            };
            if storage.get("backend").and_then(Value::as_str) != Some("local")
                || storage.get("local_path").and_then(Value::as_str) != Some(expected_root)
                || storage.get("pid").and_then(Value::as_u64)
                    != state.pid.as_deref().and_then(|pid| pid.parse().ok())
                || storage
                    .pointer("/write_fence/protocol")
                    .and_then(Value::as_str)
                    != Some(crate::queue::LocalBackend::WRITE_FENCE_PROTOCOL)
                || !mirror_matches
            {
                return Err(DeployError(format!(
                    "{label} constructed storage does not match its recorded recovery route: {storage}"
                )));
            }
            let mut correlation = if let Some(preflight) = preflight.as_ref() {
                correlate_served_store(storage_target, port, preflight, !rollback, runner).await?
            } else {
                json!({
                    "endpoint": format!("http://127.0.0.1:{port}"),
                    "object_authority": if expected_root == roots.primary { "A" } else { "B" },
                    "evidence": "constructed-runtime-without-data-mutation",
                })
            };
            correlation["runtime"] = runtime;
            let authority = correlation
                .get("object_authority")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let accepted = if rollback {
                matches!(authority, "identical")
                    || authority
                        == if roots.prior_primary == roots.primary {
                            "A"
                        } else {
                            "B"
                        }
            } else {
                matches!(authority, "A" | "identical")
            };
            if !accepted {
                return Err(DeployError(format!(
                    "{label} serves {authority:?} after {} recovery",
                    if rollback {
                        "captured-prior"
                    } else {
                        "forward A+B"
                    }
                )));
            }
            let prepared_sha256 = if rollback {
                fence.writers[index].rollback_object_recovery.as_ref()
            } else {
                fence.writers[index].forward_object_recovery.as_ref()
            }
            .map(|script| script.sha256.clone());
            Some(json!({
                "configuration": {
                    "prepared_script_sha256": prepared_sha256,
                    "loaded_environment_observed": state
                        .loaded_environment
                        .contains_key("WC_STORAGE_BACKEND"),
                    "observed_loaded_environment": state.loaded_environment.clone(),
                    "unit_declared_environment":
                        fence.writers[index].unit_declared_environment.clone(),
                    "registry_declared_environment":
                        fence.writers[index].registry_declared_environment.clone(),
                },
                "served_store": correlation,
            }))
        } else {
            None
        };
        fence.writers[index].restored_pid = state.pid;
        fence.writers[index].restored_started_at = state.process_started_at;
        fence.writers[index].restored_loaded_environment = state.loaded_environment;
        fence.writers[index].restored_executable = state.process_executable;
        fence.writers[index].restored_sha256 = state.process_sha256;
        fence.writers[index].restored_device = state.process_device;
        fence.writers[index].restored_inode = state.process_inode;
        fence.writers[index].restored_route = restored_route;
        fence.writers[index].status = "restored".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
        if fence.writers[index].role == "object-api" {
            release_storage_write_fence(
                storage_target,
                transaction,
                &mut fence,
                write_guard,
                runner,
            )
            .await?;
            let store = recovered_object_store(&fence)?;
            renew_fence_leases(&store, &mut fence).await?;
            write_fence(storage_target, transaction, &fence, runner).await?;
            restored_store = Some(store);
        } else if restored_store.is_none() {
            return Err(DeployError(
                "a writer would resume before the object API restored A and renewed every lease"
                    .to_string(),
            ));
        }
    }

    let store = restored_store.ok_or_else(|| {
        DeployError("object API did not establish the recovered authority queue".to_string())
    })?;
    release_fence_leases(storage_target, transaction, &store, &mut fence, runner).await?;
    restore_queue_control(
        storage_target,
        transaction,
        &store,
        &mut fence,
        rollback,
        runner,
    )
    .await?;
    fence.status = final_status.to_string();
    fence.restored_at = Some(Utc::now().timestamp());
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

fn validate_transaction(transaction: &str) -> Result<(), DeployError> {
    if transaction.is_empty()
        || transaction.len() > 96
        || !transaction
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(DeployError(
            "transaction must contain 1-96 ASCII letters, digits, or '-'".to_string(),
        ));
    }
    Ok(())
}

async fn remote_phase(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(phase, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}

fn read_transaction_receipt(transaction: &str) -> Result<Value, DeployError> {
    let path = transaction_directory(transaction)?.join("receipt.json");
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "transaction receipt is not a regular file: {}",
            path.display()
        )));
    }
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| DeployError(format!("cannot read {}: {error}", path.display())))?,
    )
    .map_err(|error| DeployError(format!("transaction receipt is invalid: {error}")))?;
    if receipt.get("schema").and_then(Value::as_str) != Some("stado.storage-root-reconcile.v2")
        || receipt.get("transaction").and_then(Value::as_str) != Some(transaction)
    {
        return Err(DeployError(
            "transaction receipt belongs to another reconciliation".to_string(),
        ));
    }
    Ok(receipt)
}

fn receipt_evidence_reference(
    receipt: &Value,
    field: &str,
    label: &str,
) -> Result<ImmutableEvidenceReference, DeployError> {
    serde_json::from_value(
        receipt
            .get(field)
            .cloned()
            .ok_or_else(|| DeployError(format!("receipt omitted {label} reference")))?,
    )
    .map_err(|error| DeployError(format!("receipt {label} reference is invalid: {error}")))
}

async fn typed_lifecycle_decisions(transaction: &str) -> Result<Vec<Value>, DeployError> {
    let receipt = read_transaction_receipt(transaction)?;
    let checkpoint_reference =
        receipt_evidence_reference(&receipt, "checkpoint_evidence", "checkpoint evidence")?;
    let checkpoint = read_json_evidence(
        transaction,
        CHECKPOINT_EVIDENCE_FILE,
        &checkpoint_reference,
        "checkpoint evidence",
    )?;
    if checkpoint.get("schema").and_then(Value::as_str)
        != Some("stado.storage-root-checkpoint-evidence.v1")
        || checkpoint.get("transaction").and_then(Value::as_str) != Some(transaction)
    {
        return Err(DeployError(
            "checkpoint evidence belongs to another reconciliation".to_string(),
        ));
    }
    let backup_paths = checkpoint
        .get("backup_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint evidence omitted backup objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let primary_only = checkpoint
        .get("primary_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint evidence omitted primary objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .filter(|path| !backup_paths.contains(path))
        .filter_map(|path| path.strip_prefix("ecosystem/probierz/"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let snapshot = transaction_directory(transaction)?.join("effective-lifecycle.checkpoint");
    if receipt
        .get("effective_lifecycle_checkpoint")
        .and_then(Value::as_str)
        .map(Path::new)
        != Some(snapshot.as_path())
    {
        return Err(DeployError(
            "checkpoint receipt does not name the resident immutable lifecycle snapshot"
                .to_string(),
        ));
    }
    let backend = crate::queue::LocalBackend::open_existing(&snapshot)
        .map_err(|error| DeployError(format!("cannot open lifecycle checkpoint: {error}")))?;
    let store = crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "immutable-local-snapshot",
    );
    crate::monitor::reap::classify_reconciliation_snapshot(&store, &primary_only)
        .await
        .map_err(|error| DeployError(format!("typed lifecycle snapshot refused: {error}")))
}

async fn record_typed_lifecycle_decisions(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    decisions: &[Value],
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "typed lifecycle decisions can only be recorded by the resident target worker"
                .to_string(),
        ));
    }
    write_json_evidence(
        transaction,
        LIFECYCLE_DECISIONS_FILE,
        decisions,
        "typed lifecycle decisions",
        false,
    )?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(RECORD_LIFECYCLE_DECISIONS, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}
async fn typed_final_lifecycle_observations(
    transaction: &str,
    fence: &LifecycleFence,
) -> Result<Vec<Value>, DeployError> {
    let receipt = read_transaction_receipt(transaction)?;
    let decision_reference = receipt_evidence_reference(
        &receipt,
        "lifecycle_decisions_evidence",
        "typed lifecycle decisions",
    )?;
    let decisions_value = read_json_evidence(
        transaction,
        LIFECYCLE_DECISIONS_FILE,
        &decision_reference,
        "typed lifecycle decisions",
    )?;
    let decisions = decisions_value
        .as_array()
        .ok_or_else(|| DeployError("typed lifecycle decisions are not a list".to_string()))?;
    let snapshot = transaction_directory(transaction)?.join("effective-lifecycle.checkpoint");
    let backend = crate::queue::LocalBackend::open_existing(&snapshot)
        .map_err(|error| DeployError(format!("cannot open lifecycle checkpoint: {error}")))?;
    let snapshot_store = crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "immutable-local-snapshot",
    );
    let live = recovered_object_store(fence)?;
    crate::monitor::reap::validate_reconciliation_final_state(&live, &snapshot_store, decisions)
        .await
        .map_err(|error| {
            DeployError(format!(
                "typed final lifecycle observation refused: {error}"
            ))
        })
}

async fn record_typed_final_lifecycle_observations(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    observations: &[Value],
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "typed final lifecycle observations can only be recorded by the resident target worker"
                .to_string(),
        ));
    }
    write_json_evidence(
        transaction,
        FINAL_LIFECYCLE_OBSERVATIONS_FILE,
        observations,
        "typed final lifecycle observations",
        false,
    )?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(FINALIZE, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}

fn report(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    receipt: Value,
    fence: Option<&LifecycleFence>,
) -> Result<Value, DeployError> {
    let mut report = host_channel::base_report(target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("receipt".to_string(), receipt);
    report.insert(
        "lifecycle_fence".to_string(),
        match fence {
            Some(fence) => serde_json::to_value(fence)
                .map_err(|error| DeployError(format!("cannot report lifecycle fence: {error}")))?,
            None => Value::Null,
        },
    );
    report.insert("status".to_string(), json!("ok"));
    Ok(Value::Object(report))
}

async fn reconcile_host_inner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | STATUS | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "phase must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}, not {phase:?}"
        )));
    }
    if phase == STATUS {
        let receipt = remote_phase(target, transaction, STATUS, runner).await?;
        let fence = read_fence(target, transaction, runner).await?;
        return report(target, transaction, phase, receipt, fence.as_ref());
    }
    let mut write_guard = None;
    let existing = read_fence(target, transaction, runner).await?;
    // The captured target keeps the host reachable across the outage. Its
    // managed version is not a release declaration: a remote caller may have
    // captured an older registry. Before fencing, resolve the resident host's
    // authoritative declaration; afterwards, keep the staged coordinate pinned.
    let runtime_version = match existing.as_ref() {
        Some(fence) => {
            if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
                return Err(DeployError(
                    "durable lifecycle fence belongs to another transaction".to_string(),
                ));
            }
            Some(
                fence
                    .staged_runtime
                    .as_ref()
                    .ok_or_else(|| {
                        DeployError(
                            "durable lifecycle fence omitted its staged runtime".to_string(),
                        )
                    })?
                    .request
                    .version
                    .clone(),
            )
        }
        None if matches!(phase, RUN | RESUME) => {
            let registry = crate::targets::fetch_registry_remote()
                .await
                .map_err(|error| DeployError(error.to_string()))?;
            let declared = host_channel::resolve_target(&registry, &target.name)?;
            Some(
                declared
                    .declared_version("stado")
                    .ok_or_else(|| {
                        DeployError("storage host has no declared Stado runtime".to_string())
                    })?
                    .to_string(),
            )
        }
        None => None,
    };
    let mut runtime_target = target.clone();
    if let Some(version) = runtime_version {
        runtime_target
            .managed_versions
            .insert("stado".to_string(), version);
    }
    let target = &runtime_target;
    if phase == FINALIZE {
        let mut fence =
            existing.ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
        refresh_resident_owner(target, transaction, &mut fence, runner).await?;
        if fence.status != "activated" {
            return Err(DeployError(format!(
                "finalize observes lifecycle cleanup only after activation, not {}",
                fence.status
            )));
        }
        let observations = typed_final_lifecycle_observations(transaction, &fence).await?;
        let receipt =
            record_typed_final_lifecycle_observations(target, transaction, &observations, runner)
                .await?;
        return report(target, transaction, phase, receipt, Some(&fence));
    }
    if phase == ROLLBACK
        || existing
            .as_ref()
            .is_some_and(|fence| fence.rollback_preparation)
    {
        let receipt = remote_phase(target, transaction, STATUS, runner).await?;
        let receipt_status = receipt
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut rollback_fence = existing
            .ok_or_else(|| DeployError("rollback has no recorded lifecycle fence".to_string()))?;
        if receipt_status == "absent" {
            if !rollback_fence.rollback_preparation
                && (rollback_fence.status != "preparing"
                    || !rollback_fence.queue.drained
                    || rollback_fence.lease_acquisitions.len() != rollback_fence.writers.len()
                    || rollback_fence
                        .lease_acquisitions
                        .iter()
                        .any(|entry| entry.status != "acquired" || entry.lease.is_none()))
            {
                return Err(DeployError(
                    "preparation rollback requires the recorded drained queue and complete \
                     placement leases"
                        .to_string(),
                ));
            }
            rollback_fence.rollback_preparation = true;
            write_fence(target, transaction, &rollback_fence, runner).await?;
            let fence =
                activate_lifecycle_fence(target, transaction, runner, true, &mut write_guard)
                    .await?;
            return report(
                target,
                transaction,
                phase,
                json!({
                    "schema": "stado.storage-root-reconcile.v2",
                    "transaction": transaction,
                    "status": "preparation_rolled_back",
                    "data_mutated": false,
                }),
                Some(&fence),
            );
        }
        if !matches!(
            receipt_status,
            "checkpoint_ready" | "applying" | "rollback_effects_armed"
        ) {
            return Err(DeployError(format!(
                "rollback is only safe before data activation, not receipt state {receipt_status:?}"
            )));
        }
        verify_resident_lock(transaction)?;
        acquire_storage_write_fence(
            target,
            transaction,
            &mut rollback_fence,
            &mut write_guard,
            runner,
        )
        .await?;
        let receipt = remote_phase(target, transaction, ARM_ROLLBACK, runner).await?;
        let fence =
            activate_lifecycle_fence(target, transaction, runner, true, &mut write_guard).await?;
        return report(target, transaction, phase, receipt, Some(&fence));
    }

    if existing
        .as_ref()
        .is_some_and(|fence| fence.status == "rolled_back")
    {
        return Err(DeployError(
            "a rolled-back transaction cannot be reactivated; choose a new transaction id"
                .to_string(),
        ));
    }
    let mut fence = match existing {
        Some(fence)
            if matches!(
                fence.status.as_str(),
                "activating" | "restoring" | "activated"
            ) =>
        {
            fence
        }
        _ => prepare_lifecycle_fence(target, transaction, runner, &mut write_guard).await?,
    };
    if fence.status == "fenced" {
        remote_phase(target, transaction, CHECKPOINT, runner).await?;
        let checkpoint_decisions = typed_lifecycle_decisions(transaction).await?;
        record_typed_lifecycle_decisions(target, transaction, &checkpoint_decisions, runner)
            .await?;
        fence = recheck_lifecycle_fence(target, transaction, runner).await?;
        let receipt_status = read_transaction_receipt(transaction)?
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if receipt_status != "activation_effects_armed" {
            if !matches!(
                receipt_status.as_str(),
                "checkpoint_ready" | "applying" | "data_committed_pending_activation"
            ) {
                return Err(DeployError(format!(
                    "fenced transaction has non-resumable receipt state {receipt_status:?}"
                )));
            }
            remote_phase(target, transaction, APPLY, runner).await?;
            let committed_decisions = typed_lifecycle_decisions(transaction).await?;
            if committed_decisions != checkpoint_decisions {
                return Err(DeployError(
                    "typed lifecycle decisions changed between checkpoint and data commit"
                        .to_string(),
                ));
            }
            record_typed_lifecycle_decisions(target, transaction, &committed_decisions, runner)
                .await?;
            fence = recheck_lifecycle_fence(target, transaction, runner).await?;
            remote_phase(target, transaction, ARM_ACTIVATION, runner).await?;
        }
        if read_transaction_receipt(transaction)?
            .get("status")
            .and_then(Value::as_str)
            != Some("activation_effects_armed")
        {
            return Err(DeployError(
                "activation-effect boundary was not durably recorded".to_string(),
            ));
        }
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence =
            activate_lifecycle_fence(target, transaction, runner, false, &mut write_guard).await?;
    } else if fence.status != "activated" {
        let receipt = read_transaction_receipt(transaction)?;
        if receipt.get("status").and_then(Value::as_str) != Some("activation_effects_armed") {
            return Err(DeployError(
                "partial activation has no durable activation-effect boundary".to_string(),
            ));
        }
        let decisions = typed_lifecycle_decisions(transaction).await?;
        record_typed_lifecycle_decisions(target, transaction, &decisions, runner).await?;
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence =
            activate_lifecycle_fence(target, transaction, runner, false, &mut write_guard).await?;
    }
    let receipt = remote_phase(target, transaction, ACTIVATE, runner).await?;
    report(target, transaction, phase, receipt, Some(&fence))
}
fn verify_resident_lock(transaction: &str) -> Result<(), DeployError> {
    let fd = RESIDENT_LOCK_FD.get().copied().ok_or_else(|| {
        DeployError("resident reconciliation lock descriptor is absent".to_string())
    })?;
    let lock = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    let path_metadata = std::fs::metadata(&lock)
        .map_err(|error| DeployError(format!("cannot stat {}: {error}", lock.display())))?;
    // Ask the descriptor itself. Darwin's fdesc filesystem does not promise
    // that statting `/dev/fd/N` exposes the opened object's device and inode;
    // the descriptor-authoritative `fstat(2)` does on every supported host.
    // SAFETY: the worker-owned `operation_lock` remains alive until after the
    // reconciliation outcome is recorded.
    let descriptor_metadata = nix::sys::stat::fstat(unsafe { BorrowedFd::borrow_raw(fd) })
        .map_err(|error| {
            DeployError(format!(
                "resident reconciliation lock descriptor {fd} is invalid: {error}"
            ))
        })?;
    if path_metadata.dev() as nix::libc::dev_t != descriptor_metadata.st_dev
        || path_metadata.ino() != descriptor_metadata.st_ino
    {
        return Err(DeployError(
            "resident reconciliation lock no longer maps the canonical transaction lock"
                .to_string(),
        ));
    }
    Ok(())
}

fn transaction_directory(transaction: &str) -> Result<PathBuf, DeployError> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| DeployError("resident transaction worker has no HOME".to_string()))?;
    Ok(PathBuf::from(home)
        .join(".stado/recovery/storage-root-reconcile")
        .join(transaction))
}

fn encoded_json<T: Serialize + ?Sized>(value: &T, label: &str) -> Result<Vec<u8>, DeployError> {
    let mut encoded = serde_json::to_vec(value)
        .map_err(|error| DeployError(format!("cannot encode {label}: {error}")))?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn atomic_bytes_file(path: &Path, encoded: &[u8], label: &str) -> Result<(), DeployError> {
    let parent = path
        .parent()
        .ok_or_else(|| DeployError(format!("{label} has no parent directory")))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    let parent_metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", parent.display())))?;
    if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{label} parent is not a regular directory: {}",
            parent.display()
        )));
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(format!("cannot protect {}: {error}", parent.display())))?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(DeployError(format!(
                "{label} collides with a non-regular file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DeployError(format!(
                "cannot inspect {}: {error}",
                path.display()
            )));
        }
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| DeployError(format!("{label} has an invalid file name")))?;
    let temporary = parent.join(format!(".{file_name}.{}.new", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", temporary.display())))?;
    file.write_all(encoded)
        .map_err(|error| DeployError(format!("cannot write {label}: {error}")))?;
    file.sync_all()
        .map_err(|error| DeployError(format!("cannot sync {label}: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| DeployError(format!("cannot publish {label}: {error}")))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DeployError(format!("cannot sync {}: {error}", parent.display())))?;
    Ok(())
}

fn atomic_json_file<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), DeployError> {
    atomic_bytes_file(path, &encoded_json(value, label)?, label)
}

fn evidence_reference(
    path: &Path,
    encoded: &[u8],
    label: &str,
) -> Result<ImmutableEvidenceReference, DeployError> {
    let path = path
        .to_str()
        .ok_or_else(|| DeployError(format!("{label} path is not valid UTF-8")))?
        .to_string();
    Ok(ImmutableEvidenceReference {
        path,
        sha256: hex::encode(Sha256::digest(encoded)),
        bytes: encoded.len() as u64,
    })
}

fn read_regular_file(path: &Path, label: &str) -> Result<Vec<u8>, DeployError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    std::fs::read(path)
        .map_err(|error| DeployError(format!("cannot read {}: {error}", path.display())))
}

fn write_json_evidence<T: Serialize + ?Sized>(
    transaction: &str,
    file_name: &str,
    value: &T,
    label: &str,
    replace: bool,
) -> Result<ImmutableEvidenceReference, DeployError> {
    verify_resident_lock(transaction)?;
    let path = transaction_directory(transaction)?.join(file_name);
    let encoded = encoded_json(value, label)?;
    if replace {
        atomic_bytes_file(&path, &encoded, label)?;
    } else {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                if read_regular_file(&path, label)? != encoded {
                    return Err(DeployError(format!(
                        "{label} changed after its immutable publication"
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                atomic_bytes_file(&path, &encoded, label)?;
            }
            Err(error) => {
                return Err(DeployError(format!(
                    "cannot inspect {}: {error}",
                    path.display()
                )));
            }
        }
    }
    evidence_reference(&path, &encoded, label)
}

fn read_json_evidence(
    transaction: &str,
    file_name: &str,
    reference: &ImmutableEvidenceReference,
    label: &str,
) -> Result<Value, DeployError> {
    verify_resident_lock(transaction)?;
    let path = transaction_directory(transaction)?.join(file_name);
    if path.to_str() != Some(reference.path.as_str()) {
        return Err(DeployError(format!(
            "{label} reference does not name its canonical transaction file"
        )));
    }
    let canonical = std::fs::symlink_metadata(&path)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if !canonical.file_type().is_file() || canonical.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    let mut file = std::fs::File::open(&path)
        .map_err(|error| DeployError(format!("cannot open {}: {error}", path.display())))?;
    let opened = file
        .metadata()
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if opened.dev() != canonical.dev() || opened.ino() != canonical.ino() {
        return Err(DeployError(format!(
            "{label} changed while its canonical file was opened"
        )));
    }
    let mut hasher = Sha256::new();
    let mut observed_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| DeployError(format!("cannot hash {label}: {error}")))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        observed_bytes += count as u64;
    }
    if observed_bytes != reference.bytes || hex::encode(hasher.finalize()) != reference.sha256 {
        return Err(DeployError(format!(
            "{label} bytes differ from their durable reference"
        )));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| DeployError(format!("cannot rewind {label}: {error}")))?;
    serde_json::from_reader(file)
        .map_err(|error| DeployError(format!("{label} is invalid: {error}")))
}

fn sha256_file(path: &Path) -> Result<String, DeployError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| DeployError(format!("cannot open {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DeployError(format!("cannot hash {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn atomic_owner(path: &Path, owner: &Value) -> Result<(), DeployError> {
    let parent = path
        .parent()
        .ok_or_else(|| DeployError("operation owner has no parent directory".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(format!("cannot protect {}: {error}", parent.display())))?;
    let temporary = parent.join(format!(".operation-owner.{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", temporary.display())))?;
    serde_json::to_writer(&mut file, owner)
        .map_err(|error| DeployError(format!("cannot encode operation owner: {error}")))?;
    file.write_all(b"\n")
        .map_err(|error| DeployError(format!("cannot finish operation owner: {error}")))?;
    file.sync_all()
        .map_err(|error| DeployError(format!("cannot sync operation owner: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| DeployError(format!("cannot publish operation owner: {error}")))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DeployError(format!("cannot sync {}: {error}", parent.display())))?;
    Ok(())
}

fn resident_native_manager_identity(transaction: &str) -> Result<Value, DeployError> {
    let label = format!("com.wisent.stado-storage-root-reconcile.{transaction}");
    let current_pid = std::process::id();
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/launchctl", "print", &format!("system/{label}")])
            .output()
            .map_err(|error| {
                DeployError(format!("cannot query resident launchd owner: {error}"))
            })?;
        if !output.status.success() {
            return Err(DeployError(
                "resident worker is not loaded in its captured launchd service".to_string(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let pid = stdout.lines().find_map(|line| {
            line.trim()
                .strip_prefix("pid = ")
                .and_then(|value| value.parse::<u32>().ok())
        });
        let state = stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("state = ").map(str::to_string));
        if pid != Some(current_pid) {
            return Err(DeployError(format!(
                "launchd binds the resident service to pid {pid:?}, not worker pid {current_pid}"
            )));
        }
        return Ok(json!({
            "manager": "launchd",
            "service": label,
            "domain": "system",
            "pid": current_pid,
            "state": state,
        }));
    }
    if cfg!(target_os = "linux") {
        let unit = format!("{label}.service");
        let output = std::process::Command::new("/usr/bin/sudo")
            .args([
                "-n",
                "/bin/systemctl",
                "show",
                "--property=LoadState,ActiveState,SubState,MainPID",
                &unit,
            ])
            .output()
            .map_err(|error| {
                DeployError(format!("cannot query resident systemd owner: {error}"))
            })?;
        if !output.status.success() {
            return Err(DeployError(
                "resident worker is not loaded in its captured systemd service".to_string(),
            ));
        }
        let properties = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>();
        let pid = properties
            .get("MainPID")
            .and_then(|value| value.parse::<u32>().ok());
        if properties.get("LoadState").map(String::as_str) != Some("loaded")
            || !matches!(
                properties.get("ActiveState").map(String::as_str),
                Some("active" | "activating" | "reloading")
            )
            || pid != Some(current_pid)
        {
            return Err(DeployError(format!(
                "systemd does not bind {} to worker pid {}: {:?}",
                unit, current_pid, properties
            )));
        }
        return Ok(json!({
            "manager": "systemd",
            "service": unit,
            "pid": current_pid,
            "load_state": properties.get("LoadState"),
            "active_state": properties.get("ActiveState"),
            "sub_state": properties.get("SubState"),
        }));
    }
    Err(DeployError(
        "native reconciliation worker requires Darwin launchd or Linux systemd".to_string(),
    ))
}

pub async fn reconcile_host_worker(
    target: crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    source_revision: &str,
    tool_sha256: &str,
    runner_gate: Option<Value>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    use fs2::FileExt;

    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "resident worker action must be {RUN}, {RESUME}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    if !host_channel::target_is_this_host(&target) {
        return Err(DeployError(
            "native reconciliation worker is not resident on its captured target".to_string(),
        ));
    }
    if source_revision != crate::build_identity::SOURCE_REVISION
        || source_revision == crate::build_identity::UNKNOWN_REVISION
        || source_revision.ends_with("-dirty")
    {
        return Err(DeployError(
            "resident transaction tool does not carry one clean exact source revision".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let actual_sha256 = sha256_file(&executable)?;
    if actual_sha256 != tool_sha256 {
        return Err(DeployError(
            "resident transaction tool digest differs from launch request".to_string(),
        ));
    }
    let directory = transaction_directory(transaction)?;
    let lock_path = directory
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    }
    let operation_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|error| DeployError(format!("cannot open native transaction lock: {error}")))?;
    operation_lock.try_lock_exclusive().map_err(|error| {
        DeployError(format!(
            "another reconciliation owns the native lock: {error}"
        ))
    })?;
    let descriptor = operation_lock.as_raw_fd();
    // SAFETY: `descriptor` is owned by `operation_lock`; F_GETFD/F_SETFD do
    // not consume it. Clearing CLOEXEC deliberately carries the same locked
    // open-file description through every locally spawned lifecycle effect.
    let flags = unsafe { nix::libc::fcntl(descriptor, nix::libc::F_GETFD) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                descriptor,
                nix::libc::F_SETFD,
                flags & !nix::libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(DeployError(format!(
            "cannot make native transaction lock inheritable: {}",
            std::io::Error::last_os_error()
        )));
    }
    let lock_metadata = std::fs::metadata(&lock_path)
        .map_err(|error| DeployError(format!("cannot stat native lock path: {error}")))?;
    let descriptor_metadata = operation_lock
        .metadata()
        .map_err(|error| DeployError(format!("cannot stat native lock descriptor: {error}")))?;
    if lock_metadata.dev() != descriptor_metadata.dev()
        || lock_metadata.ino() != descriptor_metadata.ino()
    {
        return Err(DeployError(
            "opened descriptor is not the canonical reconciliation lock".to_string(),
        ));
    }
    RESIDENT_LOCK_FD
        .set(descriptor)
        .map_err(|_| DeployError("resident lock descriptor was already initialized".to_string()))?;
    RESIDENT_TARGET
        .set(target.clone())
        .map_err(|_| DeployError("resident target was already initialized".to_string()))?;
    let token = uuid::Uuid::new_v4().to_string();
    RESIDENT_OWNER_TOKEN
        .set(token.clone())
        .map_err(|_| DeployError("resident owner token was already initialized".to_string()))?;
    if let Some(gate) = runner_gate {
        RESIDENT_RUNNER_GATE
            .set(gate)
            .map_err(|_| DeployError("resident runner gate was already initialized".to_string()))?;
    }
    let native_manager = resident_native_manager_identity(transaction)?;
    RESIDENT_NATIVE_MANAGER
        .set(native_manager.clone())
        .map_err(|_| {
            DeployError("resident native manager identity was already initialized".to_string())
        })?;
    let owner_path = directory.join("operation-owner.json");
    let revision = std::fs::read(&owner_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|owner| owner.get("revision").and_then(Value::as_u64))
        .unwrap_or_default()
        .saturating_add(1);
    let mut owner = json!({
        "schema": "stado.storage-root-owner.v1",
        "transaction": transaction,
        "target": target.name.clone(),
        "action": phase,
        "status": "executing",
        "pid": std::process::id(),
        "token": token,
        "source_revision": source_revision,
        "tool_path": executable,
        "tool_sha256": actual_sha256,
        "lock_device": descriptor_metadata.dev(),
        "lock_inode": descriptor_metadata.ino(),
        "native_manager": native_manager,
        "target_config": serde_json::to_value(&target)
            .map_err(|error| DeployError(format!("cannot capture resident target: {error}")))?,
        "revision": revision,
        "started_at": Utc::now().to_rfc3339(),
        "updated_at": Utc::now().to_rfc3339(),
    });
    atomic_owner(&owner_path, &owner)?;
    let outcome = reconcile_host_inner(&target, transaction, phase, runner).await;
    let fields = owner
        .as_object_mut()
        .expect("resident operation owner is an object");
    fields.insert("updated_at".to_string(), json!(Utc::now().to_rfc3339()));
    match &outcome {
        Ok(result) => {
            fields.insert("status".to_string(), json!("succeeded"));
            fields.insert("result".to_string(), result.clone());
        }
        Err(error) => {
            fields.insert("status".to_string(), json!("failed"));
            fields.insert("error".to_string(), json!(error.to_string()));
        }
    }
    atomic_owner(&owner_path, &owner)?;
    drop(operation_lock);
    outcome
}

fn launch_worker_script(
    transaction: &str,
    staged_tool: &str,
    canonical_tool: &str,
    tool_sha256: &str,
    arguments: &[String],
) -> Result<String, DeployError> {
    let arguments = serde_json::to_vec(arguments)
        .map_err(|error| DeployError(format!("cannot encode worker arguments: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(arguments);
    Ok(r##"set -euo pipefail
STADO_WORKER_ARGS=@ARGS@ STADO_STAGED_TOOL=@STAGED@ STADO_CANONICAL_TOOL=@TOOL@ STADO_TOOL_SHA256=@SHA@ STADO_TRANSACTION=@TX@ /usr/bin/python3 - <<'PY'
import base64, fcntl, hashlib, json, os, platform, plistlib, re, shlex, stat, subprocess, time
tx = os.environ["STADO_TRANSACTION"]
staged = os.path.expanduser(os.path.expandvars(os.environ["STADO_STAGED_TOOL"]))
tool = os.path.expanduser(os.path.expandvars(os.environ["STADO_CANONICAL_TOOL"]))
expected = os.environ["STADO_TOOL_SHA256"]
work = os.path.dirname(tool)
home = os.path.expanduser("~")
owner_path = os.path.join(work, "operation-owner.json")
intent_path = os.path.join(work, "launch-intent.json")
label = "com.wisent.stado-storage-root-reconcile." + tx
log_path = os.path.join(work, "transaction-worker.log")
system = platform.system()
os.makedirs(work, mode=0o700, exist_ok=True)
arguments = json.loads(base64.b64decode(os.environ["STADO_WORKER_ARGS"]))
argv = [tool] + arguments


def argument(name):
    try:
        return arguments[arguments.index(name) + 1]
    except (ValueError, IndexError):
        raise SystemExit("native worker arguments omit " + name)


captured_target = json.loads(base64.b64decode(argument("--target-config")))
requested_action = argument("--phase")
requested_revision = argument("--source-revision")

def exact_option(values, name):
    found = [
        values[index + 1]
        for index, value in enumerate(values[:-1])
        if value == name
    ]
    if len(found) != 1 or not isinstance(found[0], str):
        raise SystemExit("captured object API command must declare exactly one " + name)
    return found[0]


def captured_release_api(target):
    services = target.get("services")
    if not isinstance(services, list):
        raise SystemExit("captured target declares no service inventory")
    object_apis = [
        service for service in services
        if isinstance(service, dict)
        and service.get("label") == "com.wisent.always-on.stado-object-api"
    ]
    if len(object_apis) != 1:
        raise SystemExit("captured target must declare exactly one canonical object API")
    values = object_apis[0].get("args")
    if (not isinstance(values, list)
            or not all(isinstance(value, str) for value in values)
            or not values
            or values[0] != "dashboard"):
        raise SystemExit("captured object API command is not the dashboard")
    bind = exact_option(values, "--bind")
    port_text = exact_option(values, "--port")
    try:
        port = int(port_text)
    except ValueError:
        raise SystemExit("captured object API port is not numeric")
    if port < 1 or port > 65535:
        raise SystemExit("captured object API port is outside 1..65535")
    if bind == "::1":
        host = "[::1]"
    elif bind in ("127.0.0.1", "localhost"):
        host = bind
    else:
        raise SystemExit("captured object API release origin is not loopback")
    return "http://" + host + ":" + str(port)


release_api = captured_release_api(captured_target)


def checked(argv, accepted=(0,)):
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, close_fds=True)
    if result.returncode not in accepted:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise SystemExit(detail[-1] if detail else "native service command failed")
    return result


def atomic_json(path, value):
    temporary = path + "." + str(os.getpid()) + ".new"
    with open(temporary, "x", encoding="utf-8") as handle:
        json.dump(value, handle, sort_keys=True, separators=(",", ":"))
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    descriptor = os.open(os.path.dirname(path), os.O_RDONLY)
    os.fsync(descriptor)
    os.close(descriptor)


def read_json(path):
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            raise SystemExit(path + " is not a regular file")
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError:
        return None


def manager_state():
    if system == "Darwin":
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/launchctl", "print", "system/" + label],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        if result.returncode != 0:
            return {"manager": "launchd", "service": label, "domain": "system",
                    "loaded": False, "active": False, "starting": False, "pid": None,
                    "state": None}
        pid_match = re.search(r"(?m)^\s*pid = ([1-9][0-9]*)\s*$", result.stdout)
        state_match = re.search(r"(?m)^\s*state = (.+?)\s*$", result.stdout)
        pid = int(pid_match.group(1)) if pid_match else None
        state = state_match.group(1).strip() if state_match else None
        terminal = (state or "").lower() in ("exited", "not running")
        return {"manager": "launchd", "service": label, "domain": "system",
                "loaded": True, "active": pid is not None,
                "starting": pid is None and not terminal, "pid": pid, "state": state}
    if system == "Linux":
        unit = label + ".service"
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/systemctl", "show",
             "--property=LoadState,ActiveState,SubState,MainPID", unit],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        properties = {}
        if result.returncode == 0:
            for line in result.stdout.splitlines():
                if "=" in line:
                    key, value = line.split("=", 1)
                    properties[key] = value
        value = properties.get("MainPID", "")
        pid = int(value) if value.isdigit() and int(value) > 0 else None
        active_state = properties.get("ActiveState")
        active = pid is not None or active_state in ("active", "activating", "reloading")
        return {"manager": "systemd", "service": unit,
                "loaded": properties.get("LoadState") == "loaded",
                "active": active, "starting": active_state == "activating",
                "pid": pid, "load_state": properties.get("LoadState"),
                "active_state": active_state, "sub_state": properties.get("SubState")}
    raise SystemExit("native reconciliation worker requires Darwin launchd or Linux systemd")


def manager_bound_owner(state):
    owner = read_json(owner_path)
    if not isinstance(owner, dict):
        return None
    native = owner.get("native_manager")
    if (owner.get("schema") != "stado.storage-root-owner.v1"
            or owner.get("transaction") != tx
            or owner.get("status") != "executing"
            or not isinstance(native, dict)
            or native.get("service") != state.get("service")
            or int(owner.get("pid", 0)) != state.get("pid")
            or native.get("pid") != state.get("pid")):
        return None
    owner.pop("token", None)
    return owner


def launch_observation(state):
    intent = read_json(intent_path)
    if not isinstance(intent, dict) or intent.get("transaction") != tx:
        intent = {
            "schema": "stado.storage-root-launch.v1",
            "transaction": tx,
            "target": captured_target.get("name"),
            "target_config": captured_target,
            "action": requested_action,
            "status": "manager_starting",
        }
    intent["native_manager"] = state
    intent.pop("worker_arguments", None)
    return intent


launch_lock_path = os.path.join(
    os.path.dirname(work), "..", "storage-root-reconcile.launch.lock")
launch_lock_path = os.path.normpath(launch_lock_path)
flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
launch_lock = os.open(launch_lock_path, flags, 0o600)
fcntl.flock(launch_lock, fcntl.LOCK_EX)

state = manager_state()
if state["active"] or state["starting"]:
    owner = manager_bound_owner(state)
    observation = owner if owner is not None else launch_observation(state)
    print("STADO_RECONCILE_OWNER\t" + json.dumps(
        observation, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

operation_lock_path = os.path.normpath(os.path.join(
    os.path.dirname(work), "..", "storage-root-reconcile.lock"))
operation_lock = os.open(operation_lock_path, flags, 0o600)
try:
    fcntl.flock(operation_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    state = manager_state()
    if state["active"] or state["starting"]:
        owner = manager_bound_owner(state)
        observation = owner if owner is not None else launch_observation(state)
        print("STADO_RECONCILE_OWNER\t" + json.dumps(
            observation, sort_keys=True, separators=(",", ":")))
        raise SystemExit(0)
    raise SystemExit("native reconciliation lock is held without a manager-bound owner")

# Manager visibility and the operation lock are one observation. A unit may
# enter activating/running after the first manager read but before this
# launcher wins the lock; that transition still forbids replacement.
state = manager_state()
if state["active"] or state["starting"]:
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
    owner = manager_bound_owner(state)
    observation = owner if owner is not None else launch_observation(state)
    print("STADO_RECONCILE_OWNER\t" + json.dumps(
        observation, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

lock_info = os.fstat(operation_lock)
intent = {
    "schema": "stado.storage-root-launch.v1",
    "transaction": tx,
    "target": captured_target.get("name"),
    "target_config": captured_target,
    "action": requested_action,
    "status": "launch_intent",
    "source_revision": requested_revision,
    "tool_sha256": expected,
    "release_api": release_api,
    "native_manager": state,
    "lock_device": lock_info.st_dev,
    "lock_inode": lock_info.st_ino,
}
atomic_json(intent_path, intent)

info = os.lstat(staged)
if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
    raise SystemExit("staged transaction tool is not a regular file")
with open(staged, "rb") as handle:
    if hashlib.sha256(handle.read()).hexdigest() != expected:
        raise SystemExit("staged transaction tool digest mismatch")
os.chmod(staged, 0o700)
os.replace(staged, tool)
directory_fd = os.open(work, os.O_RDONLY)
os.fsync(directory_fd)
os.close(directory_fd)

if system == "Darwin":
    unit = {
        "Label": label,
        "ProgramArguments": argv,
        "EnvironmentVariables": {
            "HOME": home,
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "STADO_API_URL": release_api,
        },
        "WorkingDirectory": home,
        "RunAtLoad": True,
        "KeepAlive": False,
        "ProcessType": "Background",
        "UserName": checked(["/usr/bin/id", "-un"]).stdout.strip(),
        "StandardOutPath": log_path,
        "StandardErrorPath": log_path,
    }
    prepared = os.path.join(work, "native-worker.plist")
    with open(prepared + ".new", "wb") as handle:
        plistlib.dump(unit, handle, fmt=plistlib.FMT_XML, sort_keys=False)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(prepared + ".new", prepared)
    unit_path = "/Library/LaunchDaemons/" + label + ".plist"
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", "system/" + label],
            accepted=(0, 3, 113))
    checked(["/usr/bin/sudo", "-n", "/usr/bin/install", "-m", "644",
             "-o", "root", "-g", "wheel", prepared, unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "enable", "system/" + label])
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "kickstart", "system/" + label])
else:
    wrapper = os.path.join(work, "native-worker")
    with open(wrapper + ".new", "w", encoding="utf-8") as handle:
        handle.write("#!/bin/sh\nexec " + shlex.join(argv) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(wrapper + ".new", 0o700)
    os.replace(wrapper + ".new", wrapper)
    unit_path = "/etc/systemd/system/" + label + ".service"
    prepared = os.path.join(work, "native-worker.service")
    unit = "\n".join([
        "[Unit]",
        "Description=Stado storage authority reconciliation " + tx,
        "After=network-online.target",
        "[Service]",
        "Type=simple",
        "User=" + checked(["/usr/bin/id", "-un"]).stdout.strip(),
        "Environment=HOME=" + home,
        "Environment=STADO_API_URL=" + release_api,
        "WorkingDirectory=" + home,
        "ExecStart=" + wrapper,
        "Restart=no",
        "[Install]",
        "WantedBy=multi-user.target",
        "",
    ])
    with open(prepared + ".new", "w", encoding="utf-8") as handle:
        handle.write(unit)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(prepared + ".new", prepared)
    checked(["/usr/bin/sudo", "-n", "/usr/bin/install", "-m", "644",
             "-o", "root", "-g", "root", prepared, unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "daemon-reload"])
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "enable", label + ".service"])
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "start", label + ".service"])

deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    state = manager_state()
    owner = manager_bound_owner(state)
    if owner is not None:
        intent["status"] = "worker_adopted"
        intent["native_manager"] = state
        atomic_json(intent_path, intent)
        print("STADO_RECONCILE_OWNER\t" + json.dumps(
            owner, sort_keys=True, separators=(",", ":")))
        raise SystemExit(0)
    if not state["active"] and not state["starting"]:
        break
    time.sleep(0.1)
raise SystemExit("native reconciliation worker did not record manager-bound ownership")
PY"##
        .replace("@ARGS@", &shlex_quote(&encoded))
        .replace("@STAGED@", &shlex_quote(staged_tool))
        .replace("@TOOL@", &shlex_quote(canonical_tool))
        .replace("@SHA@", &shlex_quote(tool_sha256))
        .replace("@TX@", &shlex_quote(transaction)))
}

async fn read_operation_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    let output =
        host_channel::run_script(target, &bind_remote_script(READ_OWNER, transaction), runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(remote_failure_detail(
            &output,
            "operation owner could not be read",
        )));
    }
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        let Some(encoded) = line.strip_prefix("STADO_RECONCILE_OWNER\t") else {
            continue;
        };
        if encoded == "absent" {
            return Ok(None);
        }
        let mut owner: Value = serde_json::from_str(encoded)
            .map_err(|error| DeployError(format!("operation owner is invalid: {error}")))?;
        let label = owner
            .pointer("/native_manager/service")
            .and_then(Value::as_str)
            .ok_or_else(|| DeployError("operation owner omitted its native service".to_string()))?;
        let scope = match owner
            .pointer("/native_manager/domain")
            .and_then(Value::as_str)
        {
            Some("system") => service::BootoutScope::System,
            Some(domain) if domain.starts_with("gui/") || domain.starts_with("user/") => {
                service::BootoutScope::User
            }
            _ => service::BootoutScope::Any,
        };
        let observed = super::service_label_print::print_label(target, label, scope, runner).await;
        let recorded_status = owner.get("status").cloned().unwrap_or(Value::Null);
        let executing = recorded_status.as_str() == Some("executing");
        let (observation, effective_status) = match observed {
            Ok(state) => {
                let owner_running = state
                    .pid
                    .as_deref()
                    .and_then(|pid| pid.parse::<u64>().ok())
                    .is_some_and(|pid| owner.get("pid").and_then(Value::as_u64) == Some(pid));
                let effective_status = if !executing {
                    recorded_status.clone()
                } else if state.unsupported.is_some() {
                    json!("unobserved")
                } else if owner_running {
                    recorded_status.clone()
                } else {
                    json!("interrupted")
                };
                (
                    json!({
                        "observed_at": Utc::now().to_rfc3339(),
                        "loaded": state.loaded(),
                        "domain": state.domain,
                        "pid": state.pid,
                        "state": state.state,
                        "last_exit_code": state.last_exit_code,
                        "unsupported": state.unsupported,
                    }),
                    effective_status,
                )
            }
            Err(error) => (
                json!({
                    "observed_at": Utc::now().to_rfc3339(),
                    "error": error.to_string(),
                }),
                if executing {
                    json!("unobserved")
                } else {
                    recorded_status.clone()
                },
            ),
        };
        let fields = owner
            .as_object_mut()
            .ok_or_else(|| DeployError("operation owner is not an object".to_string()))?;
        fields.insert("recorded_status".to_string(), recorded_status);
        fields.insert("status".to_string(), effective_status);
        fields.insert("native_manager_observation".to_string(), observation);
        return Ok(Some(owner));
    }
    Err(DeployError(
        "operation owner reader returned no marker".to_string(),
    ))
}

fn read_captured_resident_target(
    target_name: &str,
    transaction: &str,
) -> Result<Option<crate::targets::ComputeTarget>, DeployError> {
    let directory = transaction_directory(transaction)?;
    for (name, schema) in [
        ("operation-owner.json", "stado.storage-root-owner.v1"),
        ("launch-intent.json", "stado.storage-root-launch.v1"),
    ] {
        let path = directory.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(DeployError(format!(
                    "cannot inspect captured resident target {}: {error}",
                    path.display()
                )));
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(DeployError(format!(
                "captured resident target receipt is not a regular file: {}",
                path.display()
            )));
        }
        let receipt: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|error| {
            DeployError(format!(
                "cannot read captured resident target {}: {error}",
                path.display()
            ))
        })?)
        .map_err(|error| {
            DeployError(format!(
                "captured resident target receipt {} is invalid: {error}",
                path.display()
            ))
        })?;
        if receipt.get("schema").and_then(Value::as_str) != Some(schema)
            || receipt.get("transaction").and_then(Value::as_str) != Some(transaction)
            || receipt.get("target").and_then(Value::as_str) != Some(target_name)
        {
            return Err(DeployError(format!(
                "captured resident target receipt {} has the wrong identity",
                path.display()
            )));
        }
        let target: crate::targets::ComputeTarget =
            serde_json::from_value(receipt.get("target_config").cloned().ok_or_else(|| {
                DeployError(format!(
                    "captured resident target receipt {} omitted target_config",
                    path.display()
                ))
            })?)
            .map_err(|error| {
                DeployError(format!("captured resident target is invalid: {error}"))
            })?;
        if target.name != target_name || !host_channel::target_is_this_host(&target) {
            return Err(DeployError(
                "captured resident target does not identify this host".to_string(),
            ));
        }
        return Ok(Some(target));
    }
    Ok(None)
}

pub async fn reconcile_host(
    target_name: &str,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    let target = if matches!(phase, STATUS | RESUME | ROLLBACK | FINALIZE) {
        match read_captured_resident_target(target_name, transaction)? {
            Some(target) => target,
            None => host_channel::canonical_target(target_name).await?,
        }
    } else {
        host_channel::canonical_target(target_name).await?
    };
    if phase == STATUS {
        let mut status = reconcile_host_inner(&target, transaction, STATUS, runner).await?;
        let owner = read_operation_owner(&target, transaction, runner).await?;
        status
            .as_object_mut()
            .expect("storage-root status report is an object")
            .insert("operation_owner".to_string(), owner.unwrap_or(Value::Null));
        return Ok(status);
    }
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "action must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    let runner_gate = if matches!(phase, RUN | RESUME) {
        repository_runner_gate().await?
    } else {
        None
    };
    if runner_gate.as_ref().is_some_and(|gate| {
        gate.get("source_sha").and_then(Value::as_str)
            != Some(crate::build_identity::SOURCE_REVISION)
    }) {
        return Err(DeployError(
            "current GitHub job source differs from the transaction tool source".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let tool_bytes = std::fs::read(&executable)
        .map_err(|error| DeployError(format!("cannot read transaction tool: {error}")))?;
    let tool_sha256 = hex::encode(Sha256::digest(&tool_bytes));
    let work = format!("$HOME/.stado/recovery/storage-root-reconcile/{transaction}");
    let staged_tool = format!("{work}/transaction-tool.{tool_sha256}");
    let canonical_tool = format!("{work}/transaction-tool");
    let staged =
        service::sync_service_file(&target, &staged_tool, &tool_bytes, 0o700, runner).await?;
    if !staged.succeeded("file_synced") {
        return Err(DeployError(format!(
            "transaction tool staging failed: {}",
            staged.failure()
        )));
    }
    let runner_gate = runner_gate
        .map(|gate| serde_json::to_vec(&gate))
        .transpose()
        .map_err(|error| DeployError(format!("cannot encode runner gate: {error}")))?
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_default();
    let target_config = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&target)
            .map_err(|error| DeployError(format!("cannot encode resident target: {error}")))?,
    );
    let arguments = vec![
        "host".to_string(),
        "storage-root-reconcile-worker".to_string(),
        target_name.to_string(),
        "--target-config".to_string(),
        target_config,
        "--transaction".to_string(),
        transaction.to_string(),
        "--phase".to_string(),
        phase.to_string(),
        "--source-revision".to_string(),
        crate::build_identity::SOURCE_REVISION.to_string(),
        "--tool-sha256".to_string(),
        tool_sha256.clone(),
        "--runner-gate".to_string(),
        runner_gate,
    ];
    let launched = host_channel::run_script(
        &target,
        &launch_worker_script(
            transaction,
            &staged_tool,
            &canonical_tool,
            &tool_sha256,
            &arguments,
        )?,
        runner,
    )
    .await?;
    if !launched.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &launched,
            "resident reconciliation worker did not launch",
        )));
    }
    let owner = launched
        .stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("STADO_RECONCILE_OWNER\t")
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
        })
        .ok_or_else(|| {
            DeployError("resident reconciliation worker reported no owner".to_string())
        })?;
    let mut report = host_channel::base_report(&target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("status".to_string(), json!("accepted"));
    report.insert("operation_owner".to_string(), owner);
    Ok(Value::Object(report))
}
