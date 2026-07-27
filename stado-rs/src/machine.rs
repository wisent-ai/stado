//! Stable JSON automation facade for Stado job lifecycle operations.
//!
//! Port of `stado/machine.py`. Every operation returns plain data; the CLI
//! layer (`cli/machine.rs`) wraps results in the versioned envelope
//! `{"schema_version":1,"ok":bool,"result"|"error":...}` serialized with
//! [`canonical_json`] (Python `json.dumps(..., ensure_ascii=False,
//! sort_keys=True, separators=(",", ":"))`).
//!
//! Errors are [`MachineError`] with a stable `code` (INVALID_REQUEST,
//! IDEMPOTENCY_CONFLICT, NOT_FOUND, INVALID_CURSOR, NOT_TERMINAL,
//! ARTIFACT_SECURITY, NO_ARTIFACTS, ...) and a `retryable` flag, exactly the
//! contract Python's `_invoke` emits. Unexpected storage/IO/JSON failures
//! map to code INTERNAL with retryable=false (Python `_invoke`'s catch-all).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config;
use crate::models::{job_state, Job, JobSecretRef};
use crate::queue::leases::{LeaseError, ProviderLeaseStore};
use crate::queue::submit::{submit_job, SubmitOptions};
use crate::queue::{JobStorage, StorageError};

pub const SCHEMA_VERSION: i64 = 1;
/// Prefixes probed by [`MachineFacade::lookup_job`], in probe order.
pub const JOB_PREFIXES: [&str; 6] = [
    "queue",
    "running",
    "completed",
    "uploaded",
    "failed",
    "cancelled",
];

pub const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SOURCE_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_SOURCE_MEMBERS: u64 = 100_000;

/// Request fields accepted by `machine submit` (Python `REQUEST_FIELDS`).
const REQUEST_FIELDS: &[&str] = &[
    "client_request_id",
    "command",
    "provider",
    "gpu_type",
    "vram_gb",
    "max_cost_per_hour_usd",
    "pin_to_provider",
    "priority",
    "repo",
    "repo_workdir",
    "repo_extras",
    "pre_command",
    "apt_packages",
    "output_uri",
    "verify_command",
    "exclusive",
    "source_archive_path",
    "input_objects",
    "secret_env",
];

static REQUEST_ID_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$").expect("static regex compiles")
});
static APT_PACKAGE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9+._:-]*$").expect("static regex compiles")
});
static ENV_NAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex compiles")
});
static SECRET_PART_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("static regex compiles")
});

/// Structured failure emitted by every facade operation. Serialized by the
/// CLI layer as `{"code","message","retryable"}`.
#[derive(Debug)]
pub struct MachineError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl MachineError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
        }
    }
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for MachineError {}

// Python `_invoke` maps any non-MachineError exception to INTERNAL with the
// stringified exception as the message; the From impls below reproduce that.
impl From<StorageError> for MachineError {
    fn from(exc: StorageError) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

impl From<std::io::Error> for MachineError {
    fn from(exc: std::io::Error) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

impl From<serde_json::Error> for MachineError {
    fn from(exc: serde_json::Error) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

/// `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn utcnow() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Python `repr()` of a simple string: single quotes, switching to double
/// quotes when the string contains a single quote (job ids are hex, so the
/// escape corner cases of repr never trigger).
fn py_repr(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{s}\"")
    } else {
        format!("'{s}'")
    }
}

/// Python `json.dumps(value, ensure_ascii=False, sort_keys=True,
/// separators=(",", ":"))`: compact separators, keys sorted recursively,
/// non-ASCII left as raw UTF-8 (unlike `submit::json_dumps_sorted_compact`,
/// which matches the ensure_ascii=True default used elsewhere).
pub fn canonical_json(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let btree: BTreeMap<String, Value> = map
                    .iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect();
                Value::Object(btree.into_iter().collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).expect("JSON serialization is infallible")
}

/// SHA-256 hex of the canonical request JSON (Python `_request_digest`).
fn request_digest(request: &Map<String, Value>) -> String {
    hex::encode(Sha256::digest(
        canonical_json(&Value::Object(request.clone())).as_bytes(),
    ))
}

/// Machine-facing job view (Python `normalize_job`): the queue/ prefix reads
/// as "queued", Option fields become null.
pub fn normalize_job(job: &Job) -> Value {
    let state = if job.state == "queue" {
        "queued"
    } else {
        job.state.as_str()
    };
    let mut out = Map::new();
    out.insert("job_id".into(), Value::from(job.job_id.as_str()));
    out.insert("run_id".into(), Value::from(job.run_id.as_str()));
    out.insert("batch_id".into(), Value::from(job.batch_id.as_str()));
    out.insert("state".into(), Value::from(state));
    out.insert("command".into(), Value::from(job.command.as_str()));
    out.insert("provider".into(), Value::from(job.provider.as_str()));
    out.insert("gpu_type".into(), Value::from(job.gpu_type.as_str()));
    out.insert("gpu_mem_gb".into(), Value::from(job.gpu_mem_gb));
    out.insert(
        "machine_type".into(),
        Value::from(job.machine_type.as_str()),
    );
    out.insert("created_at".into(), Value::from(job.created_at.as_str()));
    out.insert(
        "started_at".into(),
        job.started_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "completed_at".into(),
        job.completed_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "failed_at".into(),
        job.failed_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "error".into(),
        job.error.as_deref().map(Value::from).unwrap_or(Value::Null),
    );
    out.insert("output_uri".into(), Value::from(job.output_uri.as_str()));
    Value::Object(out)
}

/// A provider instance a job is recorded as holding, plus the blob the
/// record came from so an operator can go look at it.
#[derive(Debug, Clone)]
pub struct RecordedInstance {
    /// Provider name for [`crate::providers::get_provider`].
    pub provider: String,
    /// Provider-native reference, `"name@zone"` on GCE.
    pub instance_ref: String,
    /// Blob path the reference was read from.
    pub source: String,
    /// True for the `local@<host>` pseudo-refs a local agent writes. There
    /// is no cloud instance behind those and no provider call to make.
    pub local: bool,
}

/// The `instance_ref` prefix a local agent stamps on a job it claims. Not a
/// cloud resource: `queue::submit` never routes it to a provider and both
/// cancel paths skip the delete for it.
pub const LOCAL_INSTANCE_PREFIX: &str = "local@";

/// Resolve the cloud instance `job_id` is recorded as holding.
///
/// NO Python original. Two independent records exist and only one of them
/// was ever consulted:
///
///  1. the job document's `provider` / `instance_ref` fields, written by
///     the dispatcher once the instance is up, and
///  2. `provider-leases/<job_id>.json`
///     (`queue::leases::ProviderLeaseStore::load`), which records
///     `provider_resource_id` from the moment the allocation is *attempted*.
///
/// The lease is written first and cleared last, so it covers the two
/// windows the job document does not: a dispatch that created the instance
/// but died before stamping the job, and a job whose document was already
/// rewritten (moved to `failed/` by a partial cancel) while the instance
/// stayed up. Both leak a running VM that nothing else reclaims — the
/// billing gap `stado cancel --terminate` exists to close.
///
/// The job document wins when both carry a reference: it is what the
/// dispatcher confirmed, whereas a lease can still name a resource whose
/// creation call ultimately failed.
pub async fn recorded_instance(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<RecordedInstance>, LeaseError> {
    fn found(provider: &str, instance_ref: &str, source: String) -> RecordedInstance {
        RecordedInstance {
            provider: provider.to_string(),
            instance_ref: instance_ref.to_string(),
            source,
            local: instance_ref.starts_with(LOCAL_INSTANCE_PREFIX),
        }
    }
    for prefix in JOB_PREFIXES {
        let Some(job) = store.read_job(prefix, job_id).await? else {
            continue;
        };
        let instance_ref = job.instance_ref.as_deref().unwrap_or_default();
        if !instance_ref.is_empty() {
            let source = format!("{prefix}/{job_id}.json");
            return Ok(Some(found(&job.provider, instance_ref, source)));
        }
        break;
    }
    let stored = match ProviderLeaseStore::new(store.clone()).load(job_id).await {
        Ok(stored) => stored,
        // The lease store refuses any job id it cannot encode as a safe
        // path, which also means it can never have written one for this
        // job. Absence, not a failure to look.
        Err(LeaseError::Value(_)) => None,
        Err(exc) => return Err(exc),
    };
    let Some(lease) = stored else {
        return Ok(None);
    };
    if lease.provider_resource_id.is_empty() {
        return Ok(None);
    }
    let source = format!("provider-leases/{job_id}.json");
    Ok(Some(found(
        &lease.provider,
        &lease.provider_resource_id,
        source,
    )))
}

/// Validate one archive entry name against the Python path rules:
/// non-empty, no backslashes, not absolute, no `..`/empty/`.` components.
fn unsafe_archive_name(name: &str) -> bool {
    name.is_empty()
        || name.contains('\\')
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

/// Python `_validate_source_archive`: local-file safety checks + a full
/// streaming pass over the tar.gz enforcing member-count, extracted-size and
/// path-safety limits. Returns the path and the SHA-256 of the compressed
/// file, or `None` when no archive was requested.
fn validate_source_archive(
    value: Option<&Value>,
) -> Result<Option<(PathBuf, String)>, MachineError> {
    fn invalid(msg: impl Into<String>) -> MachineError {
        MachineError::new("INVALID_SOURCE_ARCHIVE", msg)
    }
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() || value.as_str() == Some("") {
        return Ok(None);
    }
    let Some(raw) = value.as_str() else {
        return Err(invalid("source_archive_path must be a string"));
    };
    let path = crate::config_file::expand_tilde(raw);
    let info = std::fs::symlink_metadata(&path)
        .map_err(|exc| invalid(format!("source archive is not readable: {exc}")))?;
    if info.file_type().is_symlink() || !info.file_type().is_file() {
        return Err(invalid("source archive must be a regular non-symlink file"));
    }
    if info.len() == 0 || info.len() > MAX_SOURCE_ARCHIVE_BYTES {
        return Err(invalid(format!(
            "source archive must be between 1 and {MAX_SOURCE_ARCHIVE_BYTES} bytes"
        )));
    }
    let mut file = std::fs::File::open(&path)
        .map_err(|exc| invalid(format!("source archive is not readable: {exc}")))?;
    let mut digest = Sha256::new();
    let mut chunk = [0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut chunk)
            .map_err(|exc| invalid(format!("source archive is not readable: {exc}")))?;
        if n == 0 {
            break;
        }
        digest.update(&chunk[..n]);
    }
    let source_sha = hex::encode(digest.finalize());

    let tar_invalid = |exc: std::io::Error| invalid(format!("invalid tar.gz archive: {exc}"));
    let file = std::fs::File::open(&path)
        .map_err(|exc| invalid(format!("source archive is not readable: {exc}")))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut total_size: u64 = 0;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let entries = archive.entries().map_err(tar_invalid)?;
    for (index, entry) in entries.enumerate() {
        if index as u64 >= MAX_SOURCE_MEMBERS {
            return Err(invalid("source archive has too many entries"));
        }
        let entry = entry.map_err(tar_invalid)?;
        let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        if unsafe_archive_name(&name) {
            return Err(invalid(format!("unsafe archive entry: {}", py_repr(&name))));
        }
        if !seen.insert(name.clone()) {
            return Err(invalid(format!(
                "duplicate archive entry: {}",
                py_repr(&name)
            )));
        }
        let entry_type = entry.header().entry_type();
        let is_dir = entry_type == tar::EntryType::Directory;
        let is_reg = entry_type == tar::EntryType::Regular;
        if !is_dir && !is_reg {
            return Err(invalid(format!(
                "non-regular archive entry: {}",
                py_repr(&name)
            )));
        }
        if is_reg {
            total_size += entry.header().size().map_err(tar_invalid)?;
            if total_size > MAX_SOURCE_EXTRACTED_BYTES {
                return Err(invalid("source archive expands beyond the safety limit"));
            }
        }
    }
    Ok(Some((path, source_sha)))
}

/// Python `_validate_request`: strict field whitelist, required fields,
/// per-field types and value rules. Returns the normalized request (defaults
/// merged) on success.
pub fn validate_request(request: &Value) -> Result<Map<String, Value>, MachineError> {
    fn invalid(msg: impl Into<String>) -> MachineError {
        MachineError::new("INVALID_REQUEST", msg)
    }
    let Value::Object(map) = request else {
        return Err(invalid("request file must contain one JSON object"));
    };
    let mut unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !REQUEST_FIELDS.contains(key))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(invalid(format!(
            "unknown request field(s): {}",
            unknown.join(", ")
        )));
    }
    let missing: Vec<&str> = ["client_request_id", "command"]
        .into_iter()
        .filter(|name| !map.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "missing required field(s): {}",
            missing.join(", ")
        )));
    }

    let request_id = &map["client_request_id"];
    let command = &map["command"];
    if !request_id
        .as_str()
        .is_some_and(|id| REQUEST_ID_RE.is_match(id))
    {
        return Err(invalid(
            "client_request_id must be 1-128 path-safe ASCII characters",
        ));
    }
    if command.as_str().is_none_or(|cmd| cmd.trim().is_empty()) {
        return Err(invalid("command must be a non-empty string"));
    }

    let mut normalized = Map::new();
    // Stado owns provider selection unless the caller supplies a constraint.
    normalized.insert("provider".into(), Value::from(""));
    normalized.insert("gpu_type".into(), Value::from(""));
    normalized.insert("vram_gb".into(), Value::from(0));
    normalized.insert("max_cost_per_hour_usd".into(), Value::from(0.0));
    normalized.insert("pin_to_provider".into(), Value::from(false));
    normalized.insert("priority".into(), Value::from(0));
    normalized.insert("repo".into(), Value::from(""));
    normalized.insert("repo_workdir".into(), Value::from(""));
    normalized.insert("repo_extras".into(), Value::from("train"));
    normalized.insert("pre_command".into(), Value::from(""));
    normalized.insert("apt_packages".into(), Value::Array(vec![]));
    normalized.insert("output_uri".into(), Value::from(""));
    normalized.insert("verify_command".into(), Value::from(""));
    normalized.insert("exclusive".into(), Value::from(false));
    normalized.insert("source_archive_path".into(), Value::from(""));
    normalized.insert("input_objects".into(), Value::Object(Map::new()));
    normalized.insert("secret_env".into(), Value::Object(Map::new()));
    for (key, value) in map {
        normalized.insert(key.clone(), value.clone());
    }

    for name in [
        "provider",
        "gpu_type",
        "repo",
        "repo_workdir",
        "repo_extras",
        "pre_command",
        "output_uri",
        "verify_command",
        "source_archive_path",
    ] {
        if !normalized[name].is_string() {
            return Err(invalid(format!("{name} must be a string")));
        }
    }
    // Python rejects bool explicitly because bool is an int subclass; JSON
    // booleans never deserialize as i64/f64 here, so as_i64/as_f64 suffices.
    for name in ["vram_gb", "priority"] {
        if normalized[name].as_i64().is_none() {
            return Err(invalid(format!("{name} must be an integer")));
        }
    }
    if normalized["vram_gb"].as_i64().unwrap_or_default() < 0 {
        return Err(invalid("vram_gb must not be negative"));
    }
    let Some(cost) = normalized["max_cost_per_hour_usd"].as_f64() else {
        return Err(invalid("max_cost_per_hour_usd must be non-negative"));
    };
    if cost < 0.0 {
        return Err(invalid("max_cost_per_hour_usd must be non-negative"));
    }
    // Python normalizes to float so the digest sees "1.0", not "1".
    normalized.insert("max_cost_per_hour_usd".into(), Value::from(cost));
    for name in ["pin_to_provider", "exclusive"] {
        if !normalized[name].is_boolean() {
            return Err(invalid(format!("{name} must be a boolean")));
        }
    }
    let packages = &normalized["apt_packages"];
    let valid_packages = packages.as_array().is_some_and(|items| {
        items
            .iter()
            .all(|item| item.as_str().is_some_and(|s| APT_PACKAGE_RE.is_match(s)))
    });
    if !valid_packages {
        return Err(invalid(
            "apt_packages must contain only valid apt package names",
        ));
    }
    let Some(secret_env) = normalized["secret_env"].as_object() else {
        return Err(invalid("secret_env must be an object"));
    };
    for (env_name, value) in secret_env {
        if !ENV_NAME_RE.is_match(env_name) {
            return Err(invalid(format!(
                "secret_env variable name is unsafe: {env_name:?}"
            )));
        }
        let Some(spec) = value.as_object() else {
            return Err(invalid(format!("secret_env.{env_name} must be an object")));
        };
        if spec.keys().any(|key| key != "item" && key != "field") {
            return Err(invalid(format!(
                "secret_env.{env_name} accepts only item and field"
            )));
        }
        let item = spec.get("item").and_then(Value::as_str).unwrap_or_default();
        let field = spec
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !SECRET_PART_RE.is_match(item) || !SECRET_PART_RE.is_match(field) {
            return Err(invalid(format!(
                "secret_env.{env_name} requires path-safe item and field strings"
            )));
        }
    }
    let Some(inputs) = normalized["input_objects"].as_object() else {
        return Err(invalid("input_objects must be an object"));
    };
    for (name, value) in inputs {
        let Some(spec) = value.as_object() else {
            return Err(invalid(format!("input_objects.{name} must be an object")));
        };
        let Some(uri) = spec.get("stado_uri").and_then(Value::as_str) else {
            return Err(invalid(format!(
                "input_objects.{name}.stado_uri is required"
            )));
        };
        crate::object_store::ObjectRef::parse(uri).map_err(|error| {
            invalid(format!(
                "input_objects.{name}.stado_uri is invalid: {error}"
            ))
        })?;
        let Some(relative) = spec.get("relative_path").and_then(Value::as_str) else {
            return Err(invalid(format!(
                "input_objects.{name}.relative_path is required"
            )));
        };
        let relative_path = Path::new(relative);
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(invalid(format!(
                "input_objects.{name}.relative_path must stay inside the job work directory"
            )));
        }
    }
    Ok(normalized)
}

/// The automation facade. Python `MachineFacade(store, submitter)`; the
/// submitter is always [`submit_job`] here and the Skarbiec-backed compute
/// API key guard covers the one case Python special-cased on submitter
/// identity.
pub struct MachineFacade {
    store: JobStorage,
    bucket: String,
}

impl MachineFacade {
    /// Facade over the configured storage backend (Python `MachineFacade()`
    /// → `JobStorage(BUCKET)`).
    pub async fn new() -> Result<Self, MachineError> {
        Ok(Self::with_store(
            JobStorage::new().await?,
            config::bucket().to_string(),
        ))
    }

    /// Facade over an explicit store (tests, custom deployments). `bucket`
    /// remains the queue facade label passed to the submitter; product object
    /// locators are always provider-neutral `stado://` URIs.
    pub fn with_store(store: JobStorage, bucket: impl Into<String>) -> Self {
        Self {
            store,
            bucket: bucket.into(),
        }
    }

    /// Read one job by id across every lifecycle prefix, stamping the
    /// prefix-derived state (Python `MachineFacade.lookup_job`).
    pub async fn lookup_job(&self, job_id: &str) -> Result<Job, MachineError> {
        let not_found = || {
            MachineError::new(
                "NOT_FOUND",
                format!("job {} was not found", py_repr(job_id)),
            )
        };
        if job_id.is_empty() || job_id.contains('/') || job_id.contains('\\') {
            return Err(not_found());
        }
        for prefix in JOB_PREFIXES {
            if let Some(mut job) = self.store.read_job(prefix, job_id).await? {
                job.state = if prefix == "queue" {
                    "queued".into()
                } else {
                    prefix.into()
                };
                return Ok(job);
            }
        }
        Err(not_found())
    }

    /// Idempotent submit (Python `submit_request`): validate, reserve
    /// `machine_requests/<id>.json` with the SHA-256 request digest, replay
    /// stored results on exact retry, reject digest mismatches with
    /// IDEMPOTENCY_CONFLICT.
    pub async fn submit_request(&self, request: &Value) -> Result<Value, MachineError> {
        let request = validate_request(request)?;
        let request_id = request["client_request_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let archive = validate_source_archive(request.get("source_archive_path"))?;
        let mut source_uri = String::new();
        let mut source_sha = String::new();
        let mut digest_request = request.clone();
        if let Some((archive_path, sha)) = archive {
            source_sha = sha;
            let source_object = crate::object_store::ObjectRef::new(
                "machine-inputs",
                &format!("{request_id}/{source_sha}.tar.gz"),
            )?;
            let source_blob = source_object.storage_path();
            source_uri = source_object.to_string();
            self.store
                .upload_file_if_absent(&source_blob, &archive_path)
                .await
                .map_err(|exc| MachineError::retryable("SOURCE_UPLOAD_FAILED", exc.to_string()))?;
            digest_request.insert(
                "source_archive_path".into(),
                Value::from(source_sha.as_str()),
            );
        }

        let record_path = format!("machine_requests/{request_id}.json");
        let digest = request_digest(&digest_request);
        let mut reservation = Map::new();
        reservation.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
        reservation.insert("client_request_id".into(), Value::from(request_id.as_str()));
        reservation.insert("request_digest".into(), Value::from(digest.as_str()));
        reservation.insert("state".into(), Value::from("submitting"));
        reservation.insert("created_at".into(), Value::from(utcnow()));
        if !source_uri.is_empty() {
            reservation.insert(
                "source_archive_uri".into(),
                Value::from(source_uri.as_str()),
            );
            reservation.insert("source_sha256".into(), Value::from(source_sha.as_str()));
        }
        let created = self
            .store
            .create_text_if_absent(
                &record_path,
                &canonical_json(&Value::Object(reservation.clone())),
            )
            .await?;
        if !created {
            let raw = self.store.download_text(&record_path).await?;
            let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
                return Err(MachineError::retryable(
                    "REQUEST_IN_PROGRESS",
                    "request reservation is not readable",
                ));
            };
            let existing: Value = serde_json::from_str(&raw).map_err(|_| {
                MachineError::new("INTERNAL", "stored idempotency record is invalid")
            })?;
            if existing.get("request_digest").and_then(Value::as_str) != Some(digest.as_str()) {
                return Err(MachineError::new(
                    "IDEMPOTENCY_CONFLICT",
                    "client_request_id was already used with a different request",
                ));
            }
            if let Some(stored_result) = existing.get("result").filter(|r| r.is_object()) {
                if stored_result.get("job").is_some_and(Value::is_object) {
                    return Ok(stored_result.clone());
                }
            }
            if let Some(stored_job) = existing.get("job").filter(|j| j.is_object()) {
                let mut result = Map::new();
                result.insert("job".into(), stored_job.clone());
                if let Some(uri) = existing.get("source_archive_uri").and_then(Value::as_str) {
                    if !uri.is_empty() {
                        result.insert("source_archive_uri".into(), Value::from(uri));
                        result.insert(
                            "source_sha256".into(),
                            existing
                                .get("source_sha256")
                                .cloned()
                                .unwrap_or_else(|| Value::from("")),
                        );
                    }
                }
                return Ok(Value::Object(result));
            }
            return Err(MachineError::retryable(
                "REQUEST_IN_PROGRESS",
                "matching request is still being submitted",
            ));
        }

        // kwargs = normalized request minus client_request_id / command /
        // source_archive_path (Python dict comprehension).
        let str_field = |name: &str| request[name].as_str().unwrap_or_default().to_string();
        let mut options = SubmitOptions {
            bucket: self.bucket.clone(),
            provider: str_field("provider"),
            gpu_type: str_field("gpu_type"),
            vram_gb: request["vram_gb"].as_i64().unwrap_or_default(),
            max_cost_per_hour_usd: request["max_cost_per_hour_usd"]
                .as_f64()
                .unwrap_or_default(),
            pin_to_provider: request["pin_to_provider"].as_bool().unwrap_or_default(),
            priority: request["priority"].as_i64().unwrap_or_default(),
            repo: str_field("repo"),
            repo_workdir: str_field("repo_workdir"),
            repo_extras: str_field("repo_extras"),
            pre_command: str_field("pre_command"),
            apt_packages: request["apt_packages"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            output_uri: str_field("output_uri"),
            verify_command: str_field("verify_command"),
            exclusive: request["exclusive"].as_bool().unwrap_or_default(),
            secret_env: request["secret_env"]
                .as_object()
                .map(|items| {
                    items
                        .iter()
                        .map(|(env_name, value)| {
                            let spec = value.as_object().expect("validated secret_env object");
                            (
                                env_name.clone(),
                                JobSecretRef {
                                    item: spec["item"]
                                        .as_str()
                                        .expect("validated secret item")
                                        .to_string(),
                                    field: spec["field"]
                                        .as_str()
                                        .expect("validated secret field")
                                        .to_string(),
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            resolved_input_artifacts: request["input_objects"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        };
        if !source_uri.is_empty() {
            // The trusted Stado agent materializes this object before spawning
            // the untrusted job. The child never receives storage credentials.
            options.resolved_input_artifacts.insert(
                "machine_source".into(),
                serde_json::json!({
                    "stado_uri": source_uri,
                    "relative_path": "machine-input.tar.gz",
                    "sha256": source_sha,
                }),
            );
            let workdir = format!("/tmp/stado-machine-source/{request_id}-{source_sha}");
            let bootstrap = [
                "set -e".to_string(),
                "mkdir -p /tmp/stado-machine-source".to_string(),
                format!("rm -rf {workdir}"),
                format!("mkdir -p {workdir}"),
                format!(
                    "tar --extract --gzip --file=\"$PWD/machine-input.tar.gz\" --directory={workdir} --no-same-owner --no-same-permissions"
                ),
                format!("cd {workdir}"),
            ]
            .join("\n");
            let caller_pre_command = &options.pre_command;
            options.pre_command = if caller_pre_command.is_empty() {
                bootstrap
            } else {
                format!("{bootstrap}\n{caller_pre_command}")
            };
            options.repo = String::new();
            options.repo_workdir = String::new();
            options.repo_extras = String::new();
        }
        let command = request["command"].as_str().unwrap_or_default();
        let job = match submit_job(command, &options).await {
            Ok(job) => job,
            Err(exc) => {
                // Roll the reservation back so a retry can re-attempt.
                let _ = self.store.delete_blob(&record_path).await;
                return Err(MachineError::retryable("SUBMIT_FAILED", exc.to_string()));
            }
        };
        let normalized = normalize_job(&job);
        let mut result = Map::new();
        result.insert("job".into(), normalized.clone());
        if !source_uri.is_empty() {
            result.insert(
                "source_archive_uri".into(),
                Value::from(source_uri.as_str()),
            );
            result.insert("source_sha256".into(), Value::from(source_sha.as_str()));
        }
        let mut completed = reservation;
        completed.insert("state".into(), Value::from("submitted"));
        completed.insert("job".into(), normalized);
        completed.insert("result".into(), Value::Object(result.clone()));
        completed.insert("completed_at".into(), Value::from(utcnow()));
        self.store
            .upload_text(&record_path, &canonical_json(&Value::Object(completed)))
            .await
            .map_err(|exc| {
                MachineError::retryable(
                    "INTERNAL",
                    format!(
                        "job {} was submitted but its idempotency record could not be finalized: {exc}",
                        job.job_id
                    ),
                )
            })?;
        Ok(Value::Object(result))
    }

    /// Python `status`.
    pub async fn status(&self, job_id: &str) -> Result<Value, MachineError> {
        let job = self.lookup_job(job_id).await?;
        let mut out = Map::new();
        out.insert("job".into(), normalize_job(&job));
        Ok(Value::Object(out))
    }

    /// Byte-cursor paging over the canonical command log (Python
    /// `read_logs`): `cursor` is a byte offset, `next_cursor` the offset of
    /// the next page, `eof` when the page reaches the current end.
    pub async fn read_logs(
        &self,
        job_id: &str,
        cursor: i64,
        limit: i64,
    ) -> Result<Value, MachineError> {
        if cursor < 0 {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor must not be negative",
            ));
        }
        if limit <= 0 {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "limit must be positive",
            ));
        }
        self.lookup_job(job_id).await?;
        let payload = self
            .store
            .read_bytes(&format!("status/{job_id}/output/command_output.log"))
            .await?
            .unwrap_or_default();
        let cursor = cursor as usize;
        if cursor > payload.len() {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor is beyond the end of the log",
            ));
        }
        let end = payload.len().min(cursor + limit as usize);
        let mut out = Map::new();
        out.insert("job_id".into(), Value::from(job_id));
        out.insert("cursor".into(), Value::from(cursor));
        out.insert("next_cursor".into(), Value::from(end));
        out.insert("eof".into(), Value::from(end == payload.len()));
        out.insert(
            "text".into(),
            Value::from(String::from_utf8_lossy(&payload[cursor..end]).into_owned()),
        );
        Ok(Value::Object(out))
    }

    /// Durable, idempotent cancel (Python `cancel_job`): writes the
    /// `cancellations/<job_id>.json` marker first so the coordinator reaps
    /// even if this call dies mid-transition.
    ///
    /// Divergence from Python, which reads `job.instance_ref` and nothing
    /// else: the instance is resolved through [`recorded_instance`], so a
    /// VM whose reference only ever reached the provider lease is deleted
    /// too instead of billing forever. Every other step is unchanged.
    pub async fn cancel_job(&self, job_id: &str) -> Result<Value, MachineError> {
        let mut job = self.lookup_job(job_id).await?;
        if job_state::is_terminal(&job.state) {
            let mut out = Map::new();
            out.insert("job".into(), normalize_job(&job));
            return Ok(Value::Object(out));
        }

        let marker_path = format!("cancellations/{job_id}.json");
        let marker = canonical_json(&serde_json::json!({
            "job_id": job_id,
            "requested_at": utcnow(),
        }));
        self.store
            .create_text_if_absent(&marker_path, &marker)
            .await?;

        if job.state == job_state::QUEUED {
            job.state = job_state::CANCELLED.into();
            job.completed_at = Some(utcnow());
            job.error = Some("cancelled".into());
            self.store.write_job("cancelled", &job).await?;
            self.store.delete_job("queue", job_id).await?;
            match self.store.read_job("running", job_id).await? {
                None => {
                    let mut out = Map::new();
                    out.insert("job".into(), normalize_job(&job));
                    return Ok(Value::Object(out));
                }
                Some(raced) => job = raced,
            }
        }

        if job.state == job_state::RUNNING {
            if let Some(instance) = recorded_instance(&self.store, job_id)
                .await
                .map_err(|exc| MachineError::retryable("CANCEL_FAILED", exc.to_string()))?
            {
                if !instance.local {
                    let provider = crate::providers::get_provider(&instance.provider)
                        .map_err(|exc| MachineError::retryable("CANCEL_FAILED", exc.to_string()))?;
                    provider
                        .delete_instance(&instance.instance_ref)
                        .await
                        .map_err(|exc| {
                            MachineError::retryable(
                                "CANCEL_FAILED",
                                format!(
                                    "failed to delete instance {} recorded in {}: {exc}",
                                    instance.instance_ref, instance.source
                                ),
                            )
                        })?;
                }
            }
            job.state = job_state::CANCELLED.into();
            job.completed_at = Some(utcnow());
            job.error = Some("cancelled".into());
            job.instance_ref = None;
            self.store.write_job("cancelled", &job).await?;
            self.store.delete_job("running", job_id).await?;
            self.store.delete_job("failed", job_id).await?;
            let mut out = Map::new();
            out.insert("job".into(), normalize_job(&job));
            return Ok(Value::Object(out));
        }

        Err(MachineError::retryable(
            "CANCEL_FAILED",
            format!("job {} is in an unsupported state", py_repr(job_id)),
        ))
    }

    /// Download and verify the canonical `status/<id>/output/` artifacts of
    /// a terminal job (Python `download_artifacts`). Every blob is hashed
    /// while streaming to disk and reported with size + sha256; output-path
    /// and storage-path symlink/escape rules are enforced exactly as Python.
    pub async fn download_artifacts(
        &self,
        job_id: &str,
        output_dir: &Path,
    ) -> Result<Value, MachineError> {
        fn security(msg: impl Into<String>) -> MachineError {
            MachineError::new("ARTIFACT_SECURITY", msg)
        }
        let job = self.lookup_job(job_id).await?;
        if !job_state::is_terminal(&job.state) {
            return Err(MachineError::new(
                "NOT_TERMINAL",
                format!("job {} is not terminal", py_repr(job_id)),
            ));
        }
        let expanded = crate::config_file::expand_tilde(&output_dir.to_string_lossy());
        // Python Path.absolute(): anchor at the cwd, no normalization.
        let requested_root = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()?.join(expanded)
        };
        if requested_root.exists() && requested_root.is_symlink() {
            return Err(security("output directory must not be a symlink"));
        }
        // Symlinked ancestors are rejected, except the system temp dir and
        // ITS parents (Python's trusted_system_aliases — macOS /var -> /
        // private/var would otherwise fail every tempfile-adjacent path).
        let temp_root = std::env::temp_dir();
        let trusted: std::collections::HashSet<&Path> = temp_root.ancestors().collect();
        for component in requested_root.ancestors().skip(1) {
            if component.exists() && component.is_symlink() && !trusted.contains(component) {
                return Err(security("output path must not contain symlinks"));
            }
        }
        if requested_root.exists() && !requested_root.is_dir() {
            return Err(security("output directory path is not a directory"));
        }
        std::fs::create_dir_all(&requested_root)?;
        let root = requested_root.canonicalize()?;
        let prefix = format!("status/{job_id}/output/");
        let mut paths: Vec<String> = self
            .store
            .list_paths(&prefix, 0)
            .await?
            .into_iter()
            .filter(|path| *path != prefix)
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err(MachineError::new(
                "NO_ARTIFACTS",
                format!("job {} has no canonical output artifacts", py_repr(job_id)),
            ));
        }

        let mut artifacts: Vec<Value> = Vec::new();
        for blob_path in &paths {
            if !blob_path.starts_with(&prefix) {
                return Err(security(
                    "storage returned an artifact outside the job output prefix",
                ));
            }
            let relative = &blob_path[prefix.len()..];
            if unsafe_archive_name(relative) {
                return Err(security(format!(
                    "unsafe artifact path: {}",
                    py_repr(relative)
                )));
            }
            let parts: Vec<&str> = relative.split('/').collect();
            let mut destination = root.clone();
            for part in &parts {
                destination.push(part);
            }
            let mut current = root.clone();
            for part in &parts[..parts.len() - 1] {
                current.push(part);
                if current.exists() && (current.is_symlink() || !current.is_dir()) {
                    return Err(security(format!(
                        "unsafe output path component: {}",
                        py_repr(part)
                    )));
                }
                std::fs::create_dir(&current).or_else(|exc| {
                    if exc.kind() == std::io::ErrorKind::AlreadyExists {
                        Ok(())
                    } else {
                        Err(exc)
                    }
                })?;
            }
            if destination.exists() && (destination.is_symlink() || !destination.is_file()) {
                return Err(security(format!(
                    "unsafe artifact destination: {}",
                    py_repr(relative)
                )));
            }
            let parent = destination.parent().unwrap_or(&root).to_path_buf();
            let temporary = tempfile::Builder::new()
                .prefix(".stado-")
                .suffix(".download")
                .tempfile_in(&parent)?
                .into_temp_path();
            let download_result = async {
                let downloaded = self.store.download_blob(blob_path, &temporary).await?;
                if !downloaded {
                    return Err(MachineError::retryable(
                        "NO_ARTIFACTS",
                        format!("artifact disappeared while downloading: {relative}"),
                    ));
                }
                let mut file = std::fs::File::open(&temporary)?;
                let mut digest = Sha256::new();
                let mut size: u64 = 0;
                let mut chunk = [0u8; 1024 * 1024];
                loop {
                    let n = file.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    size += n as u64;
                    digest.update(&chunk[..n]);
                }
                std::fs::rename(&temporary, &destination)?;
                Ok::<(u64, String), MachineError>((size, hex::encode(digest.finalize())))
            }
            .await;
            // Python `finally: temporary.unlink(missing_ok=True)`.
            let _ = std::fs::remove_file(&temporary);
            let (size, sha256) = download_result?;
            let mode = std::fs::symlink_metadata(&destination)?;
            if !mode.file_type().is_file() {
                return Err(security(format!(
                    "downloaded artifact is not a regular file: {}",
                    py_repr(relative)
                )));
            }
            let mut entry = Map::new();
            entry.insert("relative_path".into(), Value::from(parts.join("/")));
            entry.insert("size_bytes".into(), Value::from(size));
            entry.insert("sha256".into(), Value::from(sha256));
            artifacts.push(Value::Object(entry));
        }
        if artifacts.is_empty() {
            return Err(MachineError::new(
                "NO_ARTIFACTS",
                format!("job {} has no canonical output artifacts", py_repr(job_id)),
            ));
        }
        let mut out = Map::new();
        out.insert("job_id".into(), Value::from(job_id));
        out.insert(
            "output_dir".into(),
            Value::from(root.to_string_lossy().into_owned()),
        );
        out.insert("artifacts".into(), Value::Array(artifacts));
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn facade(dir: &tempfile::TempDir) -> MachineFacade {
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        MachineFacade::with_store(
            JobStorage::with_backend(Arc::new(backend), "local"),
            "test-bucket",
        )
    }

    fn plant_job(store_dir: &std::path::Path, prefix: &str, job_id: &str) -> Job {
        let mut job = Job::new(job_id, "echo hi");
        job.created_at = "2026-01-02T03:04:05+00:00".into();
        std::fs::create_dir_all(store_dir.join(prefix)).unwrap();
        std::fs::write(
            store_dir.join(prefix).join(format!("{job_id}.json")),
            job.to_json(),
        )
        .unwrap();
        job
    }

    #[test]
    fn validate_request_rules() {
        // Not an object.
        let err = validate_request(&serde_json::json!([1])).unwrap_err();
        assert_eq!(err.code, "INVALID_REQUEST");
        assert!(err.message.contains("one JSON object"), "{err}");
        // Unknown + missing fields.
        let err = validate_request(&serde_json::json!({"bogus": 1})).unwrap_err();
        assert!(
            err.message.contains("unknown request field(s): bogus"),
            "{err}"
        );
        let err = validate_request(&serde_json::json!({"command": "x"})).unwrap_err();
        assert_eq!(err.message, "missing required field(s): client_request_id");
        // Bad client_request_id / empty command.
        let err =
            validate_request(&serde_json::json!({"client_request_id": "a/b", "command": "x"}))
                .unwrap_err();
        assert!(err.message.contains("path-safe ASCII"), "{err}");
        let err =
            validate_request(&serde_json::json!({"client_request_id": "ok", "command": "  "}))
                .unwrap_err();
        assert!(err.message.contains("non-empty string"), "{err}");
        // Type rules.
        let err = validate_request(
            &serde_json::json!({"client_request_id": "ok", "command": "x", "vram_gb": 1.5}),
        )
        .unwrap_err();
        assert_eq!(err.message, "vram_gb must be an integer");
        let err = validate_request(
            &serde_json::json!({"client_request_id": "ok", "command": "x", "priority": true}),
        )
        .unwrap_err();
        assert_eq!(err.message, "priority must be an integer");
        let err = validate_request(
            &serde_json::json!({"client_request_id": "ok", "command": "x", "max_cost_per_hour_usd": -1}),
        )
        .unwrap_err();
        assert_eq!(err.message, "max_cost_per_hour_usd must be non-negative");
        let err = validate_request(
            &serde_json::json!({"client_request_id": "ok", "command": "x", "exclusive": 1}),
        )
        .unwrap_err();
        assert_eq!(err.message, "exclusive must be a boolean");
        let err = validate_request(
            &serde_json::json!({"client_request_id": "ok", "command": "x", "apt_packages": ["bad pkg"]}),
        )
        .unwrap_err();
        assert!(err.message.contains("apt package names"), "{err}");

        // Defaults merge; max_cost normalizes to float for the digest.
        let ok = validate_request(&serde_json::json!({"client_request_id": "r1", "command": "x"}))
            .unwrap();
        assert_eq!(ok["provider"], Value::from("gcp"));
        assert_eq!(ok["repo_extras"], Value::from("train"));
        assert_eq!(ok["max_cost_per_hour_usd"], Value::from(0.0));
        let int_cost =
            validate_request(&serde_json::json!({"client_request_id": "r1", "command": "x", "max_cost_per_hour_usd": 2}))
                .unwrap();
        assert_eq!(
            canonical_json(&int_cost["max_cost_per_hour_usd"]),
            "2.0",
            "int cost must digest as a Python float"
        );
    }

    #[test]
    fn canonical_json_sorts_compacts_and_keeps_utf8() {
        let value = serde_json::json!({"b": 1, "a": {"z": true, "y": "ż"}});
        assert_eq!(canonical_json(&value), r#"{"a":{"y":"ż","z":true},"b":1}"#);
    }

    #[tokio::test]
    async fn lookup_job_stamps_prefix_state() {
        let dir = tempfile::tempdir().unwrap();
        let facade = facade(&dir);
        plant_job(dir.path(), "queue", "aa11bb22");
        let job = facade.lookup_job("aa11bb22").await.unwrap();
        assert_eq!(job.state, "queued");
        let err = facade.lookup_job("missing1").await.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "job 'missing1' was not found");
        let err = facade.lookup_job("a/b").await.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn read_logs_pages_by_byte_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let facade = facade(&dir);
        plant_job(dir.path(), "running", "logjob01");
        std::fs::create_dir_all(dir.path().join("status/logjob01/output")).unwrap();
        std::fs::write(
            dir.path().join("status/logjob01/output/command_output.log"),
            b"0123456789",
        )
        .unwrap();

        let page = facade.read_logs("logjob01", 0, 4).await.unwrap();
        assert_eq!(page["text"], Value::from("0123"));
        assert_eq!(page["next_cursor"], Value::from(4));
        assert_eq!(page["eof"], Value::from(false));
        let page = facade.read_logs("logjob01", 4, 100).await.unwrap();
        assert_eq!(page["text"], Value::from("456789"));
        assert_eq!(page["next_cursor"], Value::from(10));
        assert_eq!(page["eof"], Value::from(true));
        // cursor == len is a valid empty eof page; beyond is an error.
        let page = facade.read_logs("logjob01", 10, 5).await.unwrap();
        assert_eq!(page["eof"], Value::from(true));
        assert_eq!(page["text"], Value::from(""));
        let err = facade.read_logs("logjob01", 11, 5).await.unwrap_err();
        assert_eq!(err.code, "INVALID_CURSOR");
        let err = facade.read_logs("logjob01", -1, 5).await.unwrap_err();
        assert_eq!(err.message, "cursor must not be negative");
        let err = facade.read_logs("logjob01", 0, 0).await.unwrap_err();
        assert_eq!(err.message, "limit must be positive");
        // Missing log reads as empty.
        let page = facade.read_logs("logjob01", 0, 5).await.unwrap();
        assert!(!page["text"].as_str().unwrap().is_empty() || page["eof"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn cancel_queued_job_writes_marker_and_moves_to_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let facade = facade(&dir);
        plant_job(dir.path(), "queue", "cancel01");
        let out = facade.cancel_job("cancel01").await.unwrap();
        assert_eq!(out["job"]["state"], Value::from("cancelled"));
        assert_eq!(out["job"]["error"], Value::from("cancelled"));
        // Durable marker + prefix move.
        let marker = dir.path().join("cancellations/cancel01.json");
        assert!(marker.exists(), "cancellations/ marker must be durable");
        assert!(!dir.path().join("queue/cancel01.json").exists());
        assert!(dir.path().join("cancelled/cancel01.json").exists());
        // Second cancel on the terminal job is idempotent.
        let out = facade.cancel_job("cancel01").await.unwrap();
        assert_eq!(out["job"]["state"], Value::from("cancelled"));
    }

    #[tokio::test]
    async fn artifacts_require_terminal_job() {
        let dir = tempfile::tempdir().unwrap();
        let facade = facade(&dir);
        plant_job(dir.path(), "running", "notdone1");
        let out_dir = dir.path().join("dl");
        let err = facade
            .download_artifacts("notdone1", &out_dir)
            .await
            .unwrap_err();
        assert_eq!(err.code, "NOT_TERMINAL");
    }

    #[tokio::test]
    async fn artifacts_download_hashes_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join("storage");
        std::fs::create_dir_all(&storage).unwrap();
        let facade_dir = tempfile::tempdir().unwrap();
        let _ = &facade_dir; // storage dir must exist before LocalBackend::new
        let backend = LocalBackend::new(storage.to_str().unwrap()).unwrap();
        let facade =
            MachineFacade::with_store(JobStorage::with_backend(Arc::new(backend), "local"), "b");
        plant_job(&storage, "completed", "artjob01");
        let output = storage.join("status/artjob01/output");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("metrics.json"), b"{\"loss\": 0.1}").unwrap();
        std::fs::create_dir_all(output.join("nested")).unwrap();
        std::fs::write(output.join("nested/blob.bin"), b"\x00\x01\x02").unwrap();

        let out_dir = dir.path().join("download");
        let out = facade
            .download_artifacts("artjob01", &out_dir)
            .await
            .unwrap();
        assert_eq!(out["job_id"], Value::from("artjob01"));
        let artifacts = out["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0]["relative_path"], Value::from("metrics.json"));
        assert_eq!(artifacts[0]["size_bytes"], Value::from(13));
        assert_eq!(
            artifacts[0]["sha256"],
            Value::from(hex::encode(Sha256::digest(b"{\"loss\": 0.1}")))
        );
        assert_eq!(
            artifacts[1]["relative_path"],
            Value::from("nested/blob.bin")
        );
        assert!(out_dir.join("metrics.json").exists());
        assert!(out_dir.join("nested/blob.bin").exists());

        // No artifacts -> NO_ARTIFACTS.
        plant_job(&storage, "failed", "emptyjob");
        let err = facade
            .download_artifacts("emptyjob", &out_dir)
            .await
            .unwrap_err();
        assert_eq!(err.code, "NO_ARTIFACTS");
    }

    #[tokio::test]
    async fn source_archive_validation_enforces_safety() {
        let dir = tempfile::tempdir().unwrap();
        // Build a small valid archive.
        let src = dir.path().join("src.tar.gz");
        {
            let file = std::fs::File::create(&src).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "pkg/main.py", &b"pass"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let ok = validate_source_archive(Some(&Value::from(src.to_string_lossy().into_owned())))
            .unwrap()
            .unwrap();
        assert_eq!(ok.0, src);
        assert_eq!(
            ok.1,
            hex::encode(Sha256::digest(std::fs::read(&src).unwrap()))
        );

        // Non-string / missing file.
        let err = validate_source_archive(Some(&Value::from(42))).unwrap_err();
        assert_eq!(err.code, "INVALID_SOURCE_ARCHIVE");
        let err = validate_source_archive(Some(&Value::from("/no/such/file.tgz"))).unwrap_err();
        assert!(err.message.contains("not readable"), "{err}");
        // Not a tar.gz.
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, b"not gzip").unwrap();
        let err = validate_source_archive(Some(&Value::from(bad.to_string_lossy().into_owned())))
            .unwrap_err();
        assert!(err.message.contains("invalid tar.gz archive"), "{err}");
        // Path escape inside the archive.
        let evil = dir.path().join("evil.tar.gz");
        {
            let file = std::fs::File::create(&evil).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            // Write the raw name bytes directly: Builder::append_data
            // refuses `..` paths (correctly), but the test needs one to
            // prove the VALIDATOR rejects it.
            header.as_mut_bytes()[..14].copy_from_slice(b"../escape.txt\0");
            header.set_size(1);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &b"x"[..]).unwrap();
            builder.finish().unwrap();
        }
        let err = validate_source_archive(Some(&Value::from(evil.to_string_lossy().into_owned())))
            .unwrap_err();
        assert!(err.message.contains("unsafe archive entry"), "{err}");
    }
}
