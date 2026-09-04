use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use clap::Args;
use flate2::{Compression, GzBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::CmdError;
use crate::models::{job_state, Job, JobSecretRef};
use crate::queue::storage::JobStorage;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::release_control::{
    self, QualificationStatus, ReleaseArtifactRef, ReleaseQualification, StrategyKind,
};
use crate::release_pipeline::{
    self, ArtifactReceipt, BuildReceipt, CatalogSourceIdentity, DeliveryRun, DeliveryRunState,
    PipelineChannel, PlatformRun, PlatformRunState, ProductManifest, ReceiptInput,
    ReleasePipelineManifest, ReleaseRun, ReleaseRunState, StepReceipt, StepStatus, WorkerInput,
    WorkerRequest, PRODUCT_MANIFEST,
};

const OBJECT_API_SERVICE: &str = "stado-object-api";
const OBJECT_API_REASON: &str =
    "release submission requires the canonical object store before its first write";

#[derive(Args)]
pub struct ReleaseSubmitArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    version: String,
    #[arg(long, value_enum, default_value_t = SubmitChannel::Candidate)]
    channel: SubmitChannel,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseRedeliverArgs {
    product: String,
    run_id: String,
    delivery: String,
    /// Caller-retained idempotency token for this exact redelivery attempt.
    #[arg(long)]
    retry_token: String,
    #[arg(long)]
    json: bool,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SubmitChannel {
    Candidate,
    Stable,
}
impl From<SubmitChannel> for PipelineChannel {
    fn from(v: SubmitChannel) -> Self {
        match v {
            SubmitChannel::Candidate => Self::Candidate,
            SubmitChannel::Stable => Self::Stable,
        }
    }
}
#[derive(Args)]
pub struct ReleaseWorkerArgs {
    #[arg(long, default_value = "release-request.json")]
    request: PathBuf,
}
#[derive(Args)]
pub struct DeliveryWorkerArgs {
    #[arg(long, default_value = "delivery-request.json")]
    request: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryRequest {
    schema_version: u32,
    run_id: String,
    name: String,
    product: String,
    version: String,
    platform: String,
    argv: Vec<String>,
    required: bool,
    secret_env: BTreeMap<String, String>,
    source_path: String,
    source_uri: String,
    source_sha256: String,
    archive_path: String,
    archive_uri: String,
    archive_sha256: String,
    manifest_uri: String,
    manifest_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryReceipt {
    schema_version: u32,
    run_id: String,
    job_id: String,
    name: String,
    product: String,
    version: String,
    platform: String,
    argv: Vec<String>,
    required: bool,
    secret_env: BTreeMap<String, String>,
    archive_uri: String,
    archive_sha256: String,
    manifest_uri: String,
    manifest_sha256: String,
    status: StepStatus,
    exit_code: Option<i32>,
    completed_at: String,
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, CmdError> {
    let o = Command::new("git").args(args).current_dir(root).output()?;
    if !o.status.success() {
        return Err(CmdError::click(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    Ok(o.stdout)
}
fn snapshot(root: &Path) -> Result<(String, Vec<u8>), CmdError> {
    if !git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?
    .is_empty()
    {
        return Err(CmdError::click(
            "release source must be a clean committed Git tree",
        ));
    }
    let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)
        .map_err(|_| CmdError::click("Git commit is not UTF-8"))?
        .trim()
        .to_string();
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CmdError::click("Git HEAD is not a full lowercase commit"));
    }
    if git(
        root,
        &["ls-tree", "--name-only", "HEAD", "--", PRODUCT_MANIFEST],
    )?
    .is_empty()
    {
        return Err(CmdError::click(format!(
            "{PRODUCT_MANIFEST} must be committed at HEAD"
        )));
    }
    let tar = git(root, &["archive", "--format=tar", "HEAD"])?;
    let mut gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    gz.write_all(&tar)?;
    Ok((commit, gz.finish()?))
}
async fn immutable(
    uri: &str,
    bytes: &[u8],
    kind: &str,
    meta: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    match super::storage::fetch_object_from_writer(uri).await {
        Ok(v) if v == bytes => return Ok(()),
        Ok(_) => return Err(CmdError::click(format!("immutable object differs: {uri}"))),
        Err(_) => {}
    }
    let f = tempfile::NamedTempFile::new()?;
    std::fs::write(f.path(), bytes)?;
    super::storage::store_object_with_metadata(
        uri,
        &f.path().display().to_string(),
        kind,
        true,
        meta,
    )
    .await?;
    Ok(())
}
fn run_path(product: &str, id: &str, leaf: &str) -> String {
    format!("runs/release-pipeline/{product}/{id}/{leaf}")
}
fn run_uri(product: &str, id: &str, leaf: &str) -> String {
    format!(
        "stado://{}/{}",
        crate::config::wc_stado_storage_namespace(),
        run_path(product, id, leaf)
    )
}
fn run_state_path(id: &str) -> String {
    format!("runs/release-pipeline/{id}/run.json")
}
async fn queue_immutable(path: &str, bytes: &[u8]) -> Result<(), CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(existing) = store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(CmdError::click(format!(
                "immutable queue object differs: {path}"
            )))
        };
    }
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), bytes)?;
    if store
        .upload_file_if_absent(path, file.path())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(());
    }
    match store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        Some(existing) if existing == bytes => Ok(()),
        _ => Err(CmdError::click(format!(
            "immutable queue object raced with different bytes: {path}"
        ))),
    }
}

fn deployment_receipt_identity(bytes: &[u8]) -> Result<Value, CmdError> {
    let mut receipt: Value = serde_json::from_slice(bytes)?;
    let object = receipt
        .as_object_mut()
        .ok_or_else(|| CmdError::click("deployment receipt is not an object"))?;
    if !matches!(object.remove("completed_at"), Some(Value::String(_))) {
        return Err(CmdError::click(
            "deployment receipt has no completed_at timestamp",
        ));
    }
    Ok(receipt)
}

async fn queue_deployment_receipt(path: &str, bytes: &[u8]) -> Result<(), CmdError> {
    let original_error = match queue_immutable(path, bytes).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let Some(existing) = store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Err(original_error);
    };
    if deployment_receipt_identity(&existing)? == deployment_receipt_identity(bytes)? {
        Ok(())
    } else {
        Err(original_error)
    }
}

async fn save(run: &mut ReleaseRun) -> Result<(), CmdError> {
    run.updated_at = Utc::now().to_rfc3339();
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let path = run_state_path(&run.run_id);
    let content = serde_json::to_string(run)?;
    if let Some(current) = store
        .read_text_versioned(&path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        store
            .compare_and_swap_text(&path, &current.version, &content)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        return Ok(());
    }
    if store
        .create_text_if_absent(&path, &content)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "release run state appeared concurrently: {}",
            run.run_id
        )))
    }
}
async fn persist_failure(run: &mut ReleaseRun, error: CmdError) -> CmdError {
    run.state = ReleaseRunState::Failed;
    run.failure = Some(error.to_string());
    if let Err(save_error) = save(run).await {
        return CmdError {
            message: Some(format!(
                "{}; failed to persist release failure: {}",
                error, save_error
            )),
            code: error.code,
            ..CmdError::default()
        };
    }
    error
}
async fn load(id: &str) -> Result<Option<ReleaseRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    store
        .download_text(&run_state_path(id))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|content| serde_json::from_str(&content).map_err(CmdError::from))
        .transpose()
}

/// The newest submitted run is the delivery fence. Delivery workers already
/// carry the queue storage identity needed to read run state, while fleet
/// targets deliberately do not carry a product publisher credential.
async fn latest_submitted_run(product: &str) -> Result<Option<ReleaseRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut latest: Option<(chrono::DateTime<chrono::FixedOffset>, ReleaseRun)> = None;
    for path in store
        .list_paths("runs/release-pipeline/", 0)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        if !path.ends_with("/run.json") {
            continue;
        }
        let Some(text) = store
            .download_text(&path)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        let run: ReleaseRun = serde_json::from_str(&text)
            .map_err(|error| CmdError::click(format!("invalid release run {path}: {error}")))?;
        if run.product != product {
            continue;
        }
        let created_at =
            chrono::DateTime::parse_from_rfc3339(&run.created_at).map_err(|error| {
                CmdError::click(format!(
                    "release run {} has invalid created_at: {error}",
                    run.run_id
                ))
            })?;
        let replace = match &latest {
            None => true,
            Some((latest_at, latest_run)) => {
                created_at > *latest_at
                    || (created_at == *latest_at
                        && run.run_id.as_str() > latest_run.run_id.as_str())
            }
        };
        if replace {
            latest = Some((created_at, run));
        }
    }
    Ok(latest.map(|(_, run)| run))
}

/// One run object as raw JSON, or nothing when it is absent or unreadable.
async fn load_run_value(store: &JobStorage, path: &str) -> Result<Option<Value>, CmdError> {
    let Some(text) = store
        .download_text(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Value>(&text).ok())
}

/// The most recent pipeline runs, newest first, with their persisted
/// failures.
///
/// This is the read side of the run objects `submit` maintains: `stado
/// release status` prints it and the dashboard's operator console serves the
/// same text, so a failed run is visible from the CLI and the GUI without
/// hunting through hosts or job stores.
///
/// The listing already carries each run object's write time, so the order is
/// known before a single body is downloaded and the read stops as soon as
/// `limit` matching runs are in hand. Downloading every run.json ever written
/// to then sort and truncate is the same shape as the autonomy outcome tick:
/// a bounded question answered with the whole history, over a store that
/// serves one object per HTTP request — and the release console polls this.
pub(crate) async fn recent_runs(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut ordered: Vec<String> = {
        let mut blobs = store
            .list_blobs_with_meta("runs/release-pipeline/")
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
            .into_iter()
            .filter(|blob| blob.name.ends_with("/run.json"))
            .collect::<Vec<_>>();
        blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
        blobs.into_iter().map(|blob| blob.name).collect()
    };
    let mut runs = Vec::new();
    let mut examined = usize::default();
    for path in &ordered {
        if runs.len() >= limit {
            break;
        }
        examined += true as usize;
        let Some(run) = load_run_value(&store, path).await? else {
            continue;
        };
        if product.is_some_and(|selected| run["product"].as_str() != Some(selected)) {
            continue;
        }
        runs.push(run);
    }
    // Older runs stay unread unless a live build needs its denominator; the
    // ones already consumed above cannot be that denominator.
    ordered.drain(..examined.min(ordered.len()));
    let older = ordered;
    // An in-flight run says only "publishing", which reads as a promise. The
    // run object already names each platform's queue job, and the queue knows
    // exactly where that job stands, so the two are joined here: every
    // platform of a live run carries the job's current queue state. Terminal
    // runs are left alone — their platform states are already the answer.
    for run in &mut runs {
        let live = matches!(
            run["state"].as_str(),
            Some("submitting" | "waiting" | "publishing" | "delivering")
        );
        if !live {
            continue;
        }
        let product_name = run["product"].as_str().unwrap_or("").to_owned();
        let Some(platforms) = run["platforms"].as_object_mut() else {
            continue;
        };
        for (platform_name, record) in platforms.iter_mut() {
            let Some(job_id) = record["job_id"].as_str().map(str::to_owned) else {
                continue;
            };
            for state in [
                "running",
                "queue",
                "completed",
                "uploaded",
                "failed",
                "cancelled",
            ] {
                match store.read_job(state, &job_id).await {
                    Ok(Some(_)) => {
                        record["job_state"] = Value::String(state.to_string());
                        break;
                    }
                    Ok(None) => continue,
                    Err(_) => break,
                }
            }
            // The build's own progress, from the log the agent streams while
            // the job runs: crates compiled so far, measured against the same
            // count from this platform's previous run. cargo publishes no
            // total, so the previous run IS the honest denominator, and the
            // figure is labelled an estimate everywhere it is shown.
            if record["job_state"].as_str() == Some("running") {
                if let Some(compiled) = compiling_count(&store, &job_id).await {
                    let mut progress = Map::new();
                    progress.insert("compiled".into(), Value::from(compiled));
                    if let Some(total) =
                        previous_compile_total(&store, &older, &product_name, platform_name).await
                    {
                        progress.insert("of_previous_run".into(), Value::from(total));
                        if let Some(ratio) = (compiled * 100).checked_div(total) {
                            progress.insert("percent".into(), Value::from(ratio.min(99)));
                        }
                    }
                    record["compile_progress"] = Value::Object(progress);
                }
            }
        }
    }
    Ok(runs)
}

/// Distinct crates the job's streamed log says were compiled so far.
async fn compiling_count(store: &JobStorage, job_id: &str) -> Option<u64> {
    let bytes = store
        .read_bytes(&format!("status/{job_id}/output/command_output.log"))
        .await
        .ok()
        .flatten()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(
        text.lines()
            .filter(|line| line.trim_start().starts_with("Compiling "))
            .count() as u64,
    )
}

/// The compile count of the newest older run of the same product and
/// platform whose job finished — the denominator for the estimate.
///
/// `older` is the run objects newest-first that [`recent_runs`] did not need,
/// as paths: the answer is nearly always the first or second of them, so they
/// are downloaded one at a time and the walk stops at the first usable count.
async fn previous_compile_total(
    store: &JobStorage,
    older: &[String],
    product: &str,
    platform: &str,
) -> Option<u64> {
    for path in older {
        let Ok(Some(run)) = load_run_value(store, path).await else {
            continue;
        };
        if run["product"].as_str() != Some(product) {
            continue;
        }
        let record = &run["platforms"][platform];
        let Some(job_id) = record["job_id"].as_str() else {
            continue;
        };
        if let Some(count) = compiling_count(store, job_id).await {
            if count > u64::default() {
                return Some(count);
            }
        }
    }
    None
}
fn identity(
    product: &str,
    version: &str,
    channel: PipelineChannel,
    source: &str,
    manifest: &str,
) -> String {
    release_control::sha256_bytes(
        format!("{product}\0{version}\0{channel:?}\0{source}\0{manifest}").as_bytes(),
    )[..32]
        .into()
}
/// Whether a fresh worker publication says it can accept another job.
///
/// The worker's explicit admission decision is authoritative. Missing data is
/// kept eligible during rolling upgrades because silence is not a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claimability {
    Claimable { available_cpu_cores: i64 },
    Refusing { blockers: Vec<String> },
    Unstated,
}

impl Claimability {
    /// Whether [`builder`] may pin a job here.
    pub fn eligible(&self) -> bool {
        !matches!(self, Self::Refusing { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Claimable {
                available_cpu_cores,
            } => format!("accepting jobs ({available_cpu_cores} CPU core(s) available)"),
            Self::Refusing { blockers } if blockers.is_empty() => {
                "not accepting jobs; no reason published".to_string()
            }
            Self::Refusing { blockers } => {
                format!("not accepting jobs; reasons: {}", blockers.join(", "))
            }
            Self::Unstated => "published no admission decision".to_string(),
        }
    }
}

/// Judge one publication without reaching the host.
pub fn claimability(publication: &Value) -> Claimability {
    match publication.get("accepting_jobs").and_then(Value::as_bool) {
        Some(true) => Claimability::Claimable {
            available_cpu_cores: publication
                .get("available_cpu_cores")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        },
        Some(false) => Claimability::Refusing {
            blockers: publication_blockers(publication),
        },
        None => Claimability::Unstated,
    }
}
fn publication_flag(publication: &Value, name: &str) -> Option<bool> {
    publication
        .get("diag")
        .and_then(|diag| diag.get(name))
        .and_then(Value::as_bool)
}

/// The blockers a publication itself declares, named from the shared host-gate
/// vocabulary so the selector and diagnostics cannot drift apart.
fn publication_blockers(publication: &Value) -> Vec<String> {
    use crate::deploy::host_gates::{
        DISK_CLEANUP_POLICY_UNKNOWN, DISK_CLEANUP_STALLED, DISK_PRESSURE_ACTIVE,
        DISK_PRESSURE_UNRESOLVED, QUEUE_PAUSED,
    };
    let flag = |name: &str| publication_flag(publication, name);
    let mut blockers = Vec::new();
    if flag(DISK_PRESSURE_ACTIVE) == Some(true) {
        blockers.push(format!("{DISK_PRESSURE_ACTIVE} (release deliveries only)"));
    }
    if flag(DISK_PRESSURE_UNRESOLVED) == Some(true) {
        blockers.push(DISK_PRESSURE_UNRESOLVED.to_string());
    }
    if flag("disk_cleanup_policy_known") == Some(false) {
        blockers.push(DISK_CLEANUP_POLICY_UNKNOWN.to_string());
    }
    if flag("queue_paused") == Some(true) {
        blockers.push(QUEUE_PAUSED.to_string());
    }
    // A janitor whose pass cannot start is the condition that closed both
    // darwin-arm64 builders on 2026-09-03 with ample free disk on each. The
    // publication cannot compute the gate's staleness arithmetic -- that reads
    // the janitor state file on the host -- but it does carry the outcome, and
    // `lock_busy` is the outcome that never advances `last_success_at`.
    if publication
        .get("diag")
        .and_then(|diag| diag.get("disk_cleanup"))
        .and_then(|cleanup| cleanup.get("outcome"))
        .and_then(Value::as_str)
        == Some("lock_busy")
    {
        blockers.push(format!("{DISK_CLEANUP_STALLED} (janitor pass lock_busy)"));
    }
    if let Some(reason) = publication
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str)
    {
        if !blockers.iter().any(|blocker| blocker == reason) {
            blockers.push(reason.to_string());
        }
    }
    blockers
}
async fn builder(platform: &str) -> Result<(crate::targets::ComputeTarget, String), CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let capacity = crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // Keep each live consumer's own publication, not merely its name: the
    // claimability judgement below is made from it, so no extra host read is
    // needed to know whether a candidate can take the work it would be pinned.
    let mut live_consumers = BTreeMap::new();
    for (consumer, publication) in &capacity {
        let identity = consumer.strip_prefix("local-").unwrap_or(consumer);
        if let Some(target) = registry
            .lookup_self(identity)
            .map_err(|error| CmdError::click(error.to_string()))?
        {
            live_consumers
                .entry(target.name.clone())
                .or_insert_with(|| (consumer.clone(), publication.clone()));
        }
    }
    let declared_for_platform = registry
        .targets
        .iter()
        .filter(|target| target.release_platform == platform)
        .count();
    let mut considered: Vec<(String, Claimability)> = Vec::new();
    let mut candidates: Vec<_> = registry
        .targets
        .into_iter()
        .filter_map(|target| {
            if target.release_platform != platform {
                return None;
            }
            let (consumer, publication) = live_consumers.get(&target.name)?.clone();
            let verdict = claimability(&publication);
            considered.push((target.name.clone(), verdict.clone()));
            // A host that says it will take nothing must not be pinned. The
            // pin is irrevocable for the job's lifetime, so selecting one is
            // not a slow path -- it is a job that can never run.
            verdict.eligible().then_some((target, consumer))
        })
        .collect();
    candidates.sort_by(|left, right| left.0.name.cmp(&right.0.name));
    candidates.into_iter().next().ok_or_else(|| {
        // Name the store this looked in. Builders are selected from capacity
        // publications, not from the registry's platform declaration, so a host
        // that declares the platform and publishes to a different store is
        // invisible here. This message blamed a builder that had been running
        // for seven hours, because the operator machine's queue store was a
        // private loopback resolver and the fleet publishes to a tailnet
        // address, both under namespace `probierz`.
        let store = crate::config::wc_stado_storage_url();
        let store = if store.is_empty() {
            "the configured queue store".to_string()
        } else {
            store.to_string()
        };
        // Every host that was considered and what its own publication said,
        // because "no builder is available" without a reason cost an hour of
        // nobody knowing why a pinned job never started.
        let verdicts = if considered.is_empty() {
            String::from("no declared target of that platform is publishing capacity")
        } else {
            considered
                .iter()
                .map(|(host, verdict)| format!("{host} {}", verdict.describe()))
                .collect::<Vec<_>>()
                .join("; ")
        };
        CmdError::click(format!(
            "no live fleet builder can CLAIM release_platform {platform}; capacity read \
             from {store} namespace {:?} listed {} live consumer(s) and the registry \
             declares {} target(s) for that platform. Considered: {verdicts}. A host that \
             publishes capacity but claims nothing cannot build: read \
             `stado host gates <host>` for the full verdict.",
            crate::config::wc_stado_storage_namespace(),
            live_consumers.len(),
            declared_for_platform,
        ))
    })
}

/// Resolve the consumer id last published by one exact registry target.
///
/// Delivery jobs are the recovery lane that installs a Stado version able to
/// clear a target's current gate. Requiring the target's general capacity row
/// to be fresh here deadlocks that lane: a long-running job or disk-pressure
/// gate can age the row past the scheduler horizon while the agent still has
/// enough identity to claim the exact pinned delivery. Builders still use
/// [`builder`] and therefore still require fresh, claimable capacity.
async fn target_consumer(target_name: &str) -> Result<String, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let publications = crate::queue::capacity::read_publications(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut newest = None;
    for (consumer, publication) in publications {
        let identity = consumer.strip_prefix("local-").unwrap_or(&consumer);
        let matches_target = registry
            .lookup_self(identity)
            .map_err(|error| CmdError::click(error.to_string()))?
            .is_some_and(|target| target.name == target_name);
        if !matches_target {
            continue;
        }
        let replace = newest
            .as_ref()
            .is_none_or(|(_, stamp)| publication.stamp > *stamp);
        if replace {
            newest = Some((consumer, publication.stamp));
        }
    }
    newest.map(|(consumer, _)| consumer).ok_or_else(|| {
        CmdError::click(format!(
            "delivery target {target_name} has no retained capacity publication, so its consumer \
             identity is unknown; see stado host gates {target_name}"
        ))
    })
}
fn secret_refs(v: &BTreeMap<String, String>) -> BTreeMap<String, JobSecretRef> {
    v.iter()
        .filter_map(|(n, r)| {
            r.split_once('#').map(|(i, f)| {
                (
                    n.clone(),
                    JobSecretRef {
                        item: i.into(),
                        field: f.into(),
                    },
                )
            })
        })
        .collect()
}
fn input(uri: &str, path: &str, sha: &str) -> Value {
    json!({"stado_uri":uri,"relative_path":path,"sha256":sha})
}

/// Run a release worker from the queue-owned persistent work root even when an
/// older agent materialized this job under `/tmp`.
///
/// The bridge accepts only the physical forms of the two paths the old and new
/// agents can assign to this exact canonical job id. It moves the
/// already-materialized tree before the worker starts. A compatibility symlink
/// lets the old agent's heartbeat and terminal uploader follow their hard-coded
/// path. After the worker exits, successful output is uploaded and read back
/// through the checked storage writer at both canonical status and exact
/// attempt URIs, with the attempt receipt last; any failure changes the
/// workload result to failure. A lifecycle-bound keeper uses the 0.15.10
/// `job watch --follow --json` contract: that command emits `"terminal": true`
/// even when it returns failure for a failed job. The keeper persists and
/// inspects that exact response before unlinking the verified symlink, and also
/// removes that link if the persistent workdir disappears, so an unbounded
/// artifact-finalization retry remains reachable without leaving a permanent
/// process or dangling link. Explicit log redirection and TMPDIR keep both
/// diagnostics and the old worker's build scratch in the persistent tree even
/// when `/tmp` is a separate filesystem.
const RELEASE_OUTPUT_URI_MARK: &str = "@RELEASE_OUTPUT_URI@";
const RELEASE_WORKER_COMMAND: &str = r#"set -u
umask 077
case "${WC_JOB_ID:-}" in
  job-*) job_suffix=${WC_JOB_ID#job-} ;;
  *)
    printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
    exit 1
    ;;
esac
if [ "${#job_suffix}" -ne 24 ]; then
  printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
  exit 1
fi
case "$job_suffix" in
  *[!0123456789abcdef]*)
    printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
    exit 1
    ;;
esac
old=$(/bin/pwd -P) || exit 1
home_root=$(CDPATH= cd "$HOME" && /bin/pwd -P) || exit 1
legacy_root=$(CDPATH= cd /tmp && /bin/pwd -P) || exit 1
stado_root="$home_root/.stado"
work_parent="$stado_root/work"
root="$work_parent/jobs"
work_name="wc-$WC_JOB_ID"
work="$root/$work_name"
legacy="$legacy_root/$work_name"
case "$old" in
  "$legacy"|"$work") ;;
  *)
    printf '%s\n' "[release-worker-bootstrap] refusing unexpected cwd: $old" >&2
    exit 1
    ;;
esac
job_pgid=$$
terminate_job_group() {
  trap '' TERM
  /bin/kill -TERM "-$job_pgid" 2>/dev/null || true
  /bin/sleep 2
  /bin/kill -KILL "-$job_pgid" 2>/dev/null || true
  exit 1
}
ensure_legacy_link() {
  if [ "$old" = "$work" ]; then
    return 0
  fi
  [ -d "$work" ] || return 1
  if [ -L "$old" ]; then
    [ "$(/usr/bin/readlink "$old")" = "$work" ]
    return
  fi
  if [ -e "$old" ]; then
    return 1
  fi
  /bin/ln -s "$work" "$old"
}
unlink_verified_legacy() {
  if [ -L "$old" ]; then
    [ "$(/usr/bin/readlink "$old")" = "$work" ] || return 1
    /bin/rm -f -- "$old"
    return
  fi
  [ ! -e "$old" ]
}
owner_uid=$(/usr/bin/id -u) || exit 1
owned_directory() {
  [ -d "$1" ] && [ ! -L "$1" ] || return 1
  [ "$(/usr/bin/find "$1" -prune -type d -user "$owner_uid" -print 2>/dev/null)" = "$1" ]
}
owned_regular_file() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  [ "$(/usr/bin/find "$1" -prune -type f -user "$owner_uid" -print 2>/dev/null)" = "$1" ]
}
prepare_component() {
  component="$1"
  expected="$2"
  if [ -L "$component" ]; then
    printf '%s\n' "[release-worker-bootstrap] refusing symlinked component: $expected" >&2
    return 1
  fi
  if [ ! -e "$component" ]; then
    /bin/mkdir "$component" || return 1
  fi
  [ -d "$component" ] && [ ! -L "$component" ]
}
cd "$home_root" || exit 1
[ "$(/bin/pwd -P)" = "$home_root" ] || exit 1
owned_directory . || exit 1
prepare_component .stado "$stado_root" || exit 1
cd .stado || exit 1
[ "$(/bin/pwd -P)" = "$stado_root" ] || exit 1
owned_directory . || exit 1
/bin/chmod 700 . || exit 1
prepare_component work "$work_parent" || exit 1
cd work || exit 1
[ "$(/bin/pwd -P)" = "$work_parent" ] || exit 1
owned_directory . || exit 1
/bin/chmod 700 . || exit 1
prepare_component jobs "$root" || exit 1
cd jobs || exit 1
if [ "$(/bin/pwd -P)" != "$root" ] || ! owned_directory .; then
  printf '%s\n' "[release-worker-bootstrap] persistent root changed: $root" >&2
  exit 1
fi
/bin/chmod 700 . || exit 1
if [ "$old" = "$legacy" ]; then
  if [ -e "$work_name" ] || [ -L "$work_name" ]; then
    printf '%s\n' "[release-worker-bootstrap] persistent workdir already exists: $work" >&2
    exit 1
  fi
  /bin/mv "$old" "$work_name" || exit 1
fi
if [ -L "$work_name" ] || ! owned_directory "$work_name"; then
  printf '%s\n' "[release-worker-bootstrap] unsafe persistent workdir: $work" >&2
  exit 1
fi
cd "$work_name" || exit 1
if [ "$(/bin/pwd -P)" != "$work" ] || ! owned_directory .; then
  printf '%s\n' "[release-worker-bootstrap] persistent workdir changed: $work" >&2
  exit 1
fi
/bin/chmod 700 . || exit 1
if [ "$old" = "$legacy" ]; then
  ensure_legacy_link || exit 1
fi
for child in output tmp; do
  prepare_component "$child" "$work/$child" || exit 1
  cd "$child" || exit 1
  if [ "$(/bin/pwd -P)" != "$work/$child" ] || ! owned_directory .; then
    printf '%s\n' "[release-worker-bootstrap] unsafe workdir child: $work/$child" >&2
    exit 1
  fi
  /bin/chmod 700 . || exit 1
  cd .. || exit 1
  [ "$(/bin/pwd -P)" = "$work" ] || exit 1
done
log_relative=output/command_output.log
log="$work/$log_relative"
owned_regular_file "$log_relative" || {
  printf '%s\n' "[release-worker-bootstrap] unsafe command log: $log" >&2
  exit 1
}
/bin/chmod 600 "$log_relative" || exit 1
TMPDIR="$work/tmp"
TMP="$TMPDIR"
TEMP="$TMPDIR"
export TMPDIR TMP TEMP
exec >>"$log_relative" 2>&1
attempt_output_uri=@RELEASE_OUTPUT_URI@
namespace_and_path=${attempt_output_uri#stado://}
queue_namespace=${namespace_and_path%%/*}
canonical_output_uri="stado://$queue_namespace/status/$WC_JOB_ID/output"
file_sha256() {
  digest=
  if [ -x /usr/bin/shasum ]; then
    set -- $(/usr/bin/shasum -a 256 "$1") || return 1
    digest="$1"
  elif [ -x /usr/bin/sha256sum ]; then
    set -- $(/usr/bin/sha256sum "$1") || return 1
    digest="$1"
  else
    return 1
  fi
  printf '%s' "$digest"
}
upload_output_object() {
  scope="$1"
  base_uri="$2"
  leaf="$3"
  content_type="$4"
  source="$work/output/$leaf"
  owned_regular_file "$source" || return 1
  source_sha=$(file_sha256 "$source") || return 1
  set -- $(/usr/bin/wc -c <"$source") || return 1
  source_bytes="$1"
  owned_directory "$work/tmp" || return 1
  proof=$(/usr/bin/mktemp "$work/tmp/stado-upload.XXXXXX") || return 1
  owned_regular_file "$proof" || return 1
  /bin/chmod 600 "$proof" || return 1
  if ! "$HOME/.stado/bin/stado" storage put --content-type "$content_type" --json \
    "$base_uri/$leaf" "$source" >"$proof"; then
    /bin/rm -f -- "$proof"
    return 1
  fi
  if ! owned_regular_file "$proof" ||
    ! /usr/bin/grep -Eq '^[[:space:]]*"state":[[:space:]]*"(stored|replayed)",?[[:space:]]*$' "$proof" ||
    ! /usr/bin/grep -Eq "^[[:space:]]*\"sha256\":[[:space:]]*\"$source_sha\",?[[:space:]]*$" "$proof" ||
    ! /usr/bin/grep -Eq "^[[:space:]]*\"bytes\":[[:space:]]*$source_bytes,?[[:space:]]*$" "$proof"; then
    /bin/rm -f -- "$proof"
    return 1
  fi
  /bin/rm -f -- "$proof" || return 1
  printf '%s\n' "[release-worker-bootstrap] durable_output scope=$scope leaf=$leaf sha256=$source_sha bytes=$source_bytes"
}
printf '%s\n' "[release-worker-bootstrap] workdir=$work tmpdir=$TMPDIR legacy_link=$old"
"$HOME/.stado/bin/stado" release worker --request release-request.json &
worker_pid=$!
while kill -0 "$worker_pid" 2>/dev/null; do
  if ! ensure_legacy_link; then
    printf '%s\n' "[release-worker-bootstrap] cannot preserve legacy link: $old" >&2
    terminate_job_group
  fi
  /bin/sleep 2
done
wait "$worker_pid"
rc=$?
evidence_upload_failed=0
if ! upload_output_object canonical "$canonical_output_uri" command_output.log text/plain; then
  evidence_upload_failed=1
fi
if ! upload_output_object attempt "$attempt_output_uri" command_output.log text/plain; then
  evidence_upload_failed=1
fi
if [ "$rc" -eq 0 ]; then
  if [ "$evidence_upload_failed" -eq 0 ]; then
    if ! upload_output_object canonical "$canonical_output_uri" release.tar.gz application/gzip; then
      evidence_upload_failed=1
    fi
    if ! upload_output_object attempt "$attempt_output_uri" release.tar.gz application/gzip; then
      evidence_upload_failed=1
    fi
  fi
  if [ "$evidence_upload_failed" -eq 0 ]; then
    if ! upload_output_object canonical "$canonical_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
    if [ "$evidence_upload_failed" -eq 0 ] &&
      ! upload_output_object attempt "$attempt_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
  fi
else
  if owned_regular_file "$work/output/receipt.json"; then
    if ! upload_output_object canonical "$canonical_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
    if ! upload_output_object attempt "$attempt_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
  fi
fi
if [ "$evidence_upload_failed" -ne 0 ]; then
  printf '%s\n' "[release-worker-bootstrap] output upload/read-back failed; worker_exit_code=$rc" >&2
  if [ "$rc" -eq 0 ]; then
    rc=1
  fi
fi
if ! ensure_legacy_link; then
  printf '%s\n' "[release-worker-bootstrap] cannot preserve legacy link: $old" >&2
  terminate_job_group
fi
if [ "$old" != "$work" ]; then
  (
    while [ -d "$work" ]; do
      if ! owned_directory "$work/tmp"; then
        printf '%s\n' "[release-worker-bootstrap] unsafe proof directory: $work/tmp" >&2
        break
      fi
      response=$(/usr/bin/mktemp "$work/tmp/stado-watch.XXXXXX") || {
        /bin/sleep 2
        continue
      }
      if ! owned_regular_file "$response" || ! /bin/chmod 600 "$response"; then
        /bin/rm -f -- "$response"
        /bin/sleep 2
        continue
      fi
      "$HOME/.stado/bin/stado" job watch "$WC_JOB_ID" --follow --json >"$response" &
      watch_pid=$!
      while kill -0 "$watch_pid" 2>/dev/null; do
        if [ ! -d "$work" ]; then
          /bin/kill -TERM "$watch_pid" 2>/dev/null || true
          break
        fi
        if ! ensure_legacy_link; then
          if [ ! -d "$work" ]; then
            /bin/kill -TERM "$watch_pid" 2>/dev/null || true
            break
          fi
          terminate_job_group
        fi
        /bin/sleep 2
      done
      wait "$watch_pid" 2>/dev/null || true
      if [ ! -d "$work" ]; then
        /bin/rm -f -- "$response"
        break
      fi
      if owned_regular_file "$response" &&
        /usr/bin/grep -Eq '^[[:space:]]*"terminal":[[:space:]]*true,?[[:space:]]*$' "$response"; then
        /bin/rm -f -- "$response"
        printf '%s\n' "[release-worker-bootstrap] lifecycle job_id=$WC_JOB_ID terminal=true"
        break
      fi
      /bin/rm -f -- "$response"
      if ! ensure_legacy_link; then
        [ ! -d "$work" ] && break
        terminate_job_group
      fi
      /bin/sleep 2
    done
    unlink_verified_legacy || exit 1
  ) </dev/null &
fi
exit "$rc"
"#;

// The build request's identity: every argument is a distinct coordinate the
// worker is required to receive, and each is already validated by the caller.
#[allow(clippy::too_many_arguments)]
async fn enqueue(
    store: &JobStorage,
    id: &str,
    m: &ReleasePipelineManifest,
    version: &str,
    platform: &str,
    commit: &str,
    source_sha: &str,
    source_uri: &str,
    manifest_sha: &str,
    manifest_uri: &str,
    prior_terminal_job_id: Option<&str>,
) -> Result<PlatformRun, CmdError> {
    let queue_control = crate::queue::control::read(store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if queue_control.paused {
        return Err(CmdError::click(format!(
            "release submission cannot enqueue {platform} while the queue is paused ({})",
            queue_control.pause_summary()
        )));
    }
    let recipe = &m.platforms[platform];
    let (host, consumer) = builder(&recipe.runner_platform).await?;
    let mut resolved = Map::new();
    resolved.insert(
        "source".into(),
        input(source_uri, "source.tar.gz", source_sha),
    );
    resolved.insert(
        "manifest".into(),
        input(manifest_uri, "release-manifest.json", manifest_sha),
    );
    let mut inputs = BTreeMap::new();
    for (name, v) in &m.inputs {
        let path = format!("input-archives/{name}.tar.gz");
        // Stage every declared input inside the queue namespace, which is where
        // the worker resolves objects, exactly as the source archive, manifest
        // and request are already staged.
        //
        // A cross-namespace pin such as `stado://sources/skarbiec/<sha>/...`
        // cannot be read by the worker at all: `StadoObjectBackend` builds
        // `ObjectRef::new(&self.namespace, path)`, so every read is re-prefixed
        // with the queue namespace, while `materialize_stado_inputs` hands it
        // `ecosystem/sources/...`. Publisher and worker computed different keys
        // for one object, and the build failed with `input input-skarbiec is
        // absent` naming an object that was on the store's disk the whole time.
        let bytes = super::storage::fetch_object(&v.uri).await?;
        let staged_sha = release_control::sha256_bytes(&bytes);
        if staged_sha != v.sha256 {
            return Err(CmdError::click(format!(
                "input {name} at {} hashes to {staged_sha}, recipe declares {}",
                v.uri, v.sha256
            )));
        }
        let leaf = format!("inputs/{name}.tar.gz");
        let staged_path = run_path(&m.product, id, &leaf);
        let staged_uri = run_uri(&m.product, id, &leaf);
        queue_immutable(&staged_path, &bytes).await?;
        resolved.insert(
            format!("input-{name}"),
            input(&staged_uri, &path, &v.sha256),
        );
        inputs.insert(
            name.clone(),
            WorkerInput {
                uri: staged_uri,
                sha256: v.sha256.clone(),
                archive_path: path,
                mount: v.mount.clone(),
                extract: v.extract,
            },
        );
    }
    let request = WorkerRequest {
        schema_version: 1,
        run_id: id.into(),
        product: m.product.clone(),
        version: version.into(),
        platform: platform.into(),
        builder: host.name.clone(),
        source_commit: commit.into(),
        source_sha256: source_sha.into(),
        manifest_sha256: manifest_sha.into(),
        source_archive: "source.tar.gz".into(),
        manifest_path: "release-manifest.json".into(),
        inputs,
        secret_env: recipe.secret_env.clone(),
    };
    let bytes = serde_json::to_vec(&request)?;
    let sha = release_control::sha256_bytes(&bytes);
    let request_path = run_path(&m.product, id, &format!("requests/{platform}.json"));
    let uri = run_uri(&m.product, id, &format!("requests/{platform}.json"));
    queue_immutable(&request_path, &bytes).await?;
    resolved.insert("request".into(), input(&uri, "release-request.json", &sha));
    let submission_run_id = match prior_terminal_job_id {
        Some(prior_job_id) => stable_run_id(
            "release-platform",
            &format!("{id}\0{platform}\0{prior_job_id}"),
        ),
        None => stable_run_id("release-platform", &format!("{id}\0{platform}")),
    };
    let output_uri = match prior_terminal_job_id {
        Some(_) => run_uri(
            &m.product,
            id,
            &format!("platforms/{platform}/attempts/{submission_run_id}/output"),
        ),
        None => run_uri(&m.product, id, &format!("platforms/{platform}/output")),
    };
    let command = RELEASE_WORKER_COMMAND.replace(
        RELEASE_OUTPUT_URI_MARK,
        &crate::deploy::shlex_quote(&output_uri),
    );
    let options = SubmitOptions {
        pinned_host: consumer,
        priority: crate::constants::RELEASE_JOB_PRIORITY,
        run_id: submission_run_id,
        output_uri,
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&recipe.secret_env),
        ..Default::default()
    };
    let mut jobs = submit_batch(std::slice::from_ref(&command), &options).await?;
    let job = jobs
        .pop()
        .ok_or_else(|| CmdError::click("durable release submission returned no job"))?;
    Ok(PlatformRun {
        platform: platform.into(),
        builder: host.name,
        job_id: job.job_id.clone(),
        output_prefix: format!("status/{}/output/", job.job_id),
        state: PlatformRunState::Submitted,
        artifact_sha256: None,
        release_manifest_sha256: None,
        qualification_uri: None,
        failure: None,
    })
}
async fn terminal(store: &JobStorage, id: &str) -> Result<Job, CmdError> {
    loop {
        for p in ["completed", "uploaded", "failed", "cancelled"] {
            if let Some(j) = store.read_job(p, id).await? {
                return Ok(j);
            }
        }
        if store.read_job("queue", id).await?.is_some() {
            let queue_control = crate::queue::control::read(store)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            if queue_control.paused {
                // A product release runs on the same publisher runner as a
                // Stado release. Holding that runner while maintenance keeps
                // this job queued prevents the release that can resume the
                // fleet from ever starting.
                super::cancel::cancel_in_store(store, id).await?;
                return Err(CmdError::click(format!(
                    "cancelled queued release job {id} because the queue is paused ({})",
                    queue_control.pause_summary()
                )));
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await
    }
}

/// The last lines the failed job wrote, so the failure carries its own
/// evidence.
///
/// A failed release job used to surface only the queue's one-size verdict
/// ("workload exited unsuccessfully; inspect the redacted command output"),
/// which sent the operator hunting per host. The worker names its steps in
/// that log, so its tail is the diagnosis; it travels in the CLI error and,
/// through the platform failure field, into the persisted run object the
/// dashboard serves.
async fn job_output_tail(store: &JobStorage, job_id: &str) -> String {
    let path = format!("status/{job_id}/output/command_output.log");
    match store.read_bytes(&path).await {
        Ok(Some(bytes)) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let tail = &lines[lines.len().saturating_sub(15)..];
            format!("; the job's last output:\n{}", tail.join("\n"))
        }
        Ok(None) => "; the job left no output log".to_string(),
        Err(error) => format!("; the job's output log could not be read: {error}"),
    }
}
async fn signing(product: &str) -> Result<(String, Vec<u8>), CmdError> {
    let (document, _) = super::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(product);
    let item = policy
        .map(|value| value.signing_key_item.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(crate::config::release_signing_key_item);
    let key_id = policy
        .map(|value| value.signing_key_id.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(crate::config::release_signing_key_id);
    let encoded = crate::skarbiec::read_release_signing_key(&item)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read signing key {item:?} as {}: {error}",
                crate::config::release_signing_skarbiec_consumer()
            ))
        })?
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} field private_key is required"
            ))
        })?;
    let private = BASE64
        .decode(encoded)
        .map_err(|_| CmdError::click("release signing key is not base64"))?;
    let public =
        BASE64.encode(release_control::signing_public_key(&private).map_err(CmdError::click)?);
    if control.trusted_keys.get(&key_id) != Some(&public) {
        return Err(CmdError::click(format!(
            "release key {key_id:?} is not trusted by registry"
        )));
    }
    Ok((key_id, private))
}
async fn publish(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    p: &str,
    store: &JobStorage,
    key: &str,
    private: &[u8],
) -> Result<ReleaseArtifactRef, CmdError> {
    let rec = run.platforms[p].clone();
    let job = terminal(store, &rec.job_id).await?;
    if !matches!(
        job.state.as_str(),
        job_state::COMPLETED | job_state::UPLOADED
    ) {
        let host = if job.pinned_host.is_empty() {
            "unpinned host"
        } else {
            job.pinned_host.as_str()
        };
        return Err(CmdError::click(format!(
            "release job {} ({p} on {host}) failed: {}{}",
            rec.job_id,
            job.error.clone().unwrap_or_else(|| job.state.clone()),
            job_output_tail(store, &rec.job_id).await
        )));
    }
    let prefix = format!("status/{}/output/", rec.job_id);
    let rb = store
        .read_bytes(&format!("{prefix}receipt.json"))
        .await?
        .ok_or_else(|| CmdError::click("release job omitted receipt"))?;
    let archive = store
        .read_bytes(&format!("{prefix}release.tar.gz"))
        .await?
        .ok_or_else(|| CmdError::click("release job omitted archive"))?;
    let r: BuildReceipt = serde_json::from_slice(&rb)?;
    let digest = release_control::sha256_bytes(&archive);
    if r.run_id != run.run_id
        || r.job_id != rec.job_id
        || r.product != run.product
        || r.version != run.version
        || r.platform != p
        || r.builder != rec.builder
        || r.source_commit != run.source_commit
        || r.source_sha256 != run.source_sha256
        || r.manifest_sha256 != run.manifest_sha256
        || r.status != StepStatus::Passed
        || r.artifact.as_ref().map(|v| v.sha256.as_str()) != Some(&digest)
    {
        return Err(CmdError::click(
            "release job returned mixed or invalid output",
        ));
    }
    // The runtime contract belongs to the platforms that stage it. A product
    // may now publish a platform that ships no binary at all — a web site
    // beside a CLI — and stamping that coordinate with `bin/<product>` and a
    // launcher would publish a release manifest whose binary exists in none
    // of its own bytes. The rollout side never reaches such a platform (a
    // target names the platform it rolls out, in the product's rollout
    // policy), so the wrong claim would sit in the published manifest
    // unread until something believed it.
    let runtime = m
        .platforms
        .get(p)
        .filter(|recipe| {
            matches!(
                release_pipeline::platform_runtime_role(recipe, m.runtime.as_ref()),
                release_pipeline::RuntimeRole::Runtime
            )
        })
        .and(m.runtime.as_ref());
    let q = ReleaseQualification {
        status: QualificationStatus::Passed,
        evidence_sha256: Some(release_control::sha256_bytes(&rb)),
        completed_at: Some(r.completed_at),
    };
    let (a, _) =
        super::release_cmd::publish_pipeline_release(super::release_cmd::PipelinePublishRequest {
            product: &run.product,
            version: &run.version,
            platform: p,
            archive: &archive,
            source_revision: &run.source_commit,
            source_sha256: &run.source_sha256,
            pipeline_manifest_sha256: &run.manifest_sha256,
            binary: runtime.map(|v| v.binary.as_str()).unwrap_or(""),
            launcher: runtime.map(|v| v.launcher.as_str()).unwrap_or(""),
            config_schema: runtime.map(|v| v.config_schema).unwrap_or(0),
            state_schema: runtime.map(|v| v.state_schema).unwrap_or(0),
            minimum_stado_version: runtime
                .map(|v| v.minimum_stado_version.as_str())
                .unwrap_or(""),
            rollback_compatible_with: runtime
                .map(|v| v.rollback_compatible_with.as_slice())
                .unwrap_or(&[]),
            qualification: q,
            qualification_receipt: &rb,
            key_id: key,
            private_key: private,
            builder: &rec.builder,
        })
        .await?;
    let u = run.platforms.get_mut(p).unwrap();
    u.state = PlatformRunState::Published;
    u.artifact_sha256 = Some(a.artifact_sha256.clone());
    u.release_manifest_sha256 = Some(a.manifest_sha256.clone());
    u.qualification_uri = Some(format!(
        "stado://releases/{}/{}/{}/{}",
        run.product,
        run.version,
        p,
        release_control::RELEASE_QUALIFICATION_NAME
    ));
    Ok(a)
}

/// Refuse a release whose runtime declaration omits the version it would
/// replace, before anything is built, signed or published.
///
/// The host enforces the same rule at rollout, and enforcing it only there is
/// expensive: `release_agent` quarantines the candidate digest as
/// `rollback_compatibility_undeclared`, and a quarantined immutable coordinate
/// is spent -- the version can be abandoned but never retried, because a
/// rebuild of the same version writes different bytes to a coordinate that
/// refuses to differ. Brama burnt 0.2.40, 0.2.44, 0.2.54 and 0.2.59 exactly
/// that way, each time because a hand-kept list had not been told about the
/// release that shipped before it. Both sides of the comparison are readable
/// here, one document read before the first build, so the answer arrives while
/// it is still free and names the edit that fixes it.
async fn require_rollback_compatibility(
    manifest: &ReleasePipelineManifest,
    version: &str,
) -> Result<(), CmdError> {
    let Some(runtime) = manifest.runtime.as_ref() else {
        return Ok(());
    };
    let (document, _) = super::registry::fetch_versioned_document().await?;
    let Some(control) = release_control::control(&document)? else {
        return Ok(());
    };
    let Some(policy) = control.products.get(&manifest.product) else {
        return Ok(());
    };
    let Some(desired) = policy.desired.as_ref() else {
        return Ok(());
    };
    if desired.version == version
        || runtime
            .rollback_compatible_with
            .iter()
            .any(|declared| declared == &desired.version)
    {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} {version} does not declare rollback compatibility with {}, the release it would \
         replace; add \"{}\" to runtime.rollback_compatible_with in {PRODUCT_MANIFEST}. Without \
         it every rollout target quarantines this digest and the coordinate is spent.",
        manifest.product, desired.version, desired.version
    )))
}

pub async fn submit(args: &ReleaseSubmitArgs) -> Result<(), CmdError> {
    let root = args.source.canonicalize()?;
    let manifest_bytes = std::fs::read(root.join(PRODUCT_MANIFEST))?;
    let pm = release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?;
    let ProductManifest::Release(m) = pm.clone() else {
        return Err(CmdError::click("product declares releases:false"));
    };
    let declared =
        release_pipeline::declared_version(&root, &m.version_source).map_err(CmdError::click)?;
    if declared != args.version {
        return Err(CmdError::click(
            "--version disagrees with declared version source",
        ));
    }
    let channel = args.channel.into();
    if !m.promotion.channels.contains(&channel) {
        return Err(CmdError::click(
            "requested channel is forbidden by promotion policy",
        ));
    }
    require_rollback_compatibility(&m, &args.version).await?;
    // An explicit endpoint may be a Stado-managed loopback forward to the
    // control host. Only an absent endpoint means this caller owns the local
    // object daemon and must ensure it before publishing.
    if crate::config::stado_api_url().is_empty()
        && crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            != Some(crate::capabilities::StorageAdapter::Local)
    {
        super::service::ensure_local_dependency(OBJECT_API_SERVICE, OBJECT_API_REASON, true)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot ensure required service {OBJECT_API_SERVICE}: {error}"
                ))
            })?;
    }
    let (commit, archive) = snapshot(&root)?;
    // Reserve every platform coordinate before this submission can become the
    // newest durable run. Delivery workers fence themselves against that
    // newest run. When the claim lived only in `publish`, a second source tree
    // could persist a newer run for the same version, fail later against the
    // first tree's immutable claim, and still make every valid delivery from
    // the first run refuse itself as superseded. Claiming in the manifest's
    // stable platform order makes that incompatible submission fail before it
    // can become a delivery fence.
    for platform in m.platforms.keys() {
        super::release_cmd::claim_release_coordinate(&m.product, &args.version, platform, &commit)
            .await?;
    }
    let source_sha = release_control::sha256_bytes(&archive);
    let manifest_sha = release_control::sha256_bytes(&manifest_bytes);
    let source_uri = format!("stado://sources/{}/{}/source.tar.gz", m.product, source_sha);
    let meta = BTreeMap::from([
        ("stado-source-commit".into(), commit.clone()),
        ("stado-source-sha256".into(), source_sha.clone()),
        ("stado-manifest-sha256".into(), manifest_sha.clone()),
    ]);
    immutable(&source_uri, &archive, "application/gzip", &meta).await?;
    super::release_catalog::publish_entry(
        pm,
        manifest_sha.clone(),
        Some(CatalogSourceIdentity {
            commit: commit.clone(),
            source_sha256: source_sha.clone(),
            source_uri: source_uri.clone(),
        }),
    )
    .await?;
    let id = identity(
        &m.product,
        &args.version,
        channel,
        &source_sha,
        &manifest_sha,
    );
    let source_input_path = run_path(&m.product, &id, "inputs/source.tar.gz");
    let source_input_uri = run_uri(&m.product, &id, "inputs/source.tar.gz");
    queue_immutable(&source_input_path, &archive).await?;
    let manifest_path = run_path(&m.product, &id, "manifest.json");
    let manifest_uri = run_uri(&m.product, &id, "manifest.json");
    queue_immutable(&manifest_path, &manifest_bytes).await?;
    let now = Utc::now().to_rfc3339();
    let mut run = load(&id).await?.unwrap_or(ReleaseRun {
        schema_version: 1,
        run_id: id.clone(),
        product: m.product.clone(),
        version: args.version.clone(),
        channel,
        source_commit: commit.clone(),
        source_sha256: source_sha.clone(),
        source_uri: source_uri.clone(),
        manifest_sha256: manifest_sha.clone(),
        manifest_uri: manifest_uri.clone(),
        state: ReleaseRunState::Submitting,
        platforms: BTreeMap::new(),
        deliveries: BTreeMap::new(),
        failure: None,
        created_at: now.clone(),
        updated_at: now,
    });
    if run.source_commit != commit
        || run.source_sha256 != source_sha
        || run.manifest_sha256 != manifest_sha
    {
        return Err(CmdError::click("durable release run identity mismatch"));
    }
    run.failure = None;
    save(&mut run).await?;
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_failure(&mut run, CmdError::click(error.to_string())).await)
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    let mut enqueue_failure = None;
    for p in &platforms {
        if run.platforms.contains_key(p) {
            let base = release_control::release_base(&m.product, &args.version, p)
                .map_err(CmdError::click)?;
            let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
            // release.json is the coordinate's commit marker. A process may
            // publish it and lose the next status write; rebuilding then creates
            // a different qualification timestamp and collides with the
            // immutable coordinate. Recover only after the complete signed
            // coordinate verifies and names this exact source revision.
            if super::storage::release_object_present(&manifest_uri).await? {
                let artifact =
                    super::release_cmd::verified_artifact_for_submit(&m.product, &args.version, p)
                        .await?;
                if artifact.source_revision != commit {
                    return Err(CmdError::click(format!(
                        "published release {p} names source revision {}, expected {commit}",
                        artifact.source_revision
                    )));
                }
                {
                    let platform = run.platforms.get_mut(p).expect("checked above");
                    platform.state = PlatformRunState::Published;
                    platform.artifact_sha256 = Some(artifact.artifact_sha256);
                    platform.release_manifest_sha256 = Some(artifact.manifest_sha256);
                    platform.qualification_uri = Some(format!(
                        "{base}/{}",
                        release_control::RELEASE_QUALIFICATION_NAME
                    ));
                    platform.failure = None;
                }
                save(&mut run).await?;
                continue;
            }
            // A run may say Published while its coordinate was written through
            // an obsolete object origin. The durable qualification job is still
            // the source of truth; republish it instead of treating absent
            // release.json as a committed release.
            if run.platforms[p].state == PlatformRunState::Published {
                let platform = run.platforms.get_mut(p).expect("checked above");
                platform.state = PlatformRunState::Qualified;
                platform.artifact_sha256 = None;
                platform.release_manifest_sha256 = None;
                platform.qualification_uri = None;
                platform.failure = None;
                save(&mut run).await?;
            }
        }
        if !run.platforms.contains_key(p) || run.platforms[p].state == PlatformRunState::Failed {
            let prior_terminal_job_id = run
                .platforms
                .get(p)
                .filter(|platform| platform.state == PlatformRunState::Failed)
                .map(|platform| platform.job_id.as_str());
            let r = match enqueue(
                &store,
                &id,
                &m,
                &args.version,
                p,
                &commit,
                &source_sha,
                &source_input_uri,
                &manifest_sha,
                &manifest_uri,
                prior_terminal_job_id,
            )
            .await
            {
                Ok(run) => run,
                Err(error) => {
                    enqueue_failure = Some(error);
                    break;
                }
            };
            run.platforms.insert(p.clone(), r);
            save(&mut run).await?
        }
    }
    let submitted_platforms: Vec<_> = platforms
        .iter()
        .filter(|platform| run.platforms.contains_key(*platform))
        .cloned()
        .collect();
    run.state = ReleaseRunState::Waiting;
    save(&mut run).await?;
    let (key, private) = match signing(&run.product).await {
        Ok(signing) => signing,
        Err(error) => return Err(persist_failure(&mut run, error).await),
    };
    run.state = ReleaseRunState::Publishing;
    save(&mut run).await?;
    let mut artifacts = BTreeMap::new();
    for p in &submitted_platforms {
        let result = if run.platforms[p].state == PlatformRunState::Published {
            super::release_cmd::verified_artifact_for_submit(&run.product, &run.version, p).await
        } else {
            publish(&mut run, &m, p, &store, &key, &private).await
        };
        let a = match result {
            Ok(artifact) => artifact,
            Err(error) => {
                let platform = run.platforms.get_mut(p).unwrap();
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(error.to_string());
                if m.platforms[p].required {
                    return Err(persist_failure(&mut run, error).await);
                }
                save(&mut run).await?;
                continue;
            }
        };
        save(&mut run).await?;
        artifacts.insert(p.clone(), a);
    }
    if let Some(error) = enqueue_failure {
        return Err(persist_failure(&mut run, error).await);
    }
    run.state = ReleaseRunState::Delivering;
    save(&mut run).await?;
    if let Err(error) = run_deliveries(&mut run, &m, &artifacts).await {
        return Err(persist_failure(&mut run, error).await);
    }
    if m.promotion.reconcile {
        if let Err(error) =
            super::release_cmd::promote_for_submit(&run.product, &run.version, run.channel).await
        {
            return Err(persist_failure(&mut run, error).await);
        }
        if let Err(error) = reconcile(&run).await {
            return Err(persist_failure(&mut run, error).await);
        }
        run.state = ReleaseRunState::Reconciled
    } else {
        run.state = ReleaseRunState::Completed
    }
    save(&mut run).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&run)?)
    } else {
        println!(
            "release run {} product={} version={} state={:?}",
            run.run_id, run.product, run.version, run.state
        )
    }
    Ok(())
}

fn delivery_job_command(product: &str) -> &'static str {
    if product == "stado" {
        crate::constants::RELEASE_DELIVERY_JOB_COMMAND
    } else {
        crate::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
    }
}

async fn run_deliveries(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    for d in &m.deliveries {
        // A failed delivery is re-enqueued the same way a failed platform is
        // re-built: the record's existence is not the work's existence. Until
        // 2026-08-19 a resumed run skipped every recorded delivery whatever
        // its state, so a run whose required deliveries had all failed
        // marked itself Completed — done without checking the world.
        if !run.deliveries.contains_key(&d.name)
            || run.deliveries[&d.name].state == DeliveryRunState::Failed
        {
            let a = &artifacts[&d.platform];
            let request = DeliveryRequest {
                schema_version: 1,
                run_id: run.run_id.clone(),
                name: d.name.clone(),
                product: run.product.clone(),
                version: run.version.clone(),
                platform: d.platform.clone(),
                argv: d.argv.clone(),
                required: d.required,
                secret_env: d.secret_env.clone(),
                source_path: "source.tar.gz".into(),
                source_uri: run.source_uri.clone(),
                source_sha256: run.source_sha256.clone(),
                archive_path: "release.tar.gz".into(),
                archive_uri: a.archive_uri.clone(),
                archive_sha256: a.artifact_sha256.clone(),
                manifest_uri: a.manifest_uri.clone(),
                manifest_sha256: a.manifest_sha256.clone(),
            };
            let bytes = serde_json::to_vec(&request)?;
            let sha = release_control::sha256_bytes(&bytes);
            let uri = run_uri(
                &run.product,
                &run.run_id,
                &format!("deliveries/{}/request.json", d.name),
            );
            queue_immutable(
                &run_path(
                    &run.product,
                    &run.run_id,
                    &format!("deliveries/{}/request.json", d.name),
                ),
                &bytes,
            )
            .await?;
            let mut resolved = Map::new();
            resolved.insert("request".into(), input(&uri, "delivery-request.json", &sha));
            resolved.insert(
                "archive".into(),
                input(&a.archive_uri, "release.tar.gz", &a.artifact_sha256),
            );
            resolved.insert(
                "source".into(),
                input(
                    &run_uri(&run.product, &run.run_id, "inputs/source.tar.gz"),
                    "source.tar.gz",
                    &run.source_sha256,
                ),
            );
            // A delivery that names its target runs ON that target and
            // installs locally; only target-less deliveries fall back to any
            // live builder of the platform.
            let consumer = if d.target.is_empty() {
                builder(&m.platforms[&d.platform].runner_platform).await?.1
            } else {
                target_consumer(&d.target).await?
            };
            let options = SubmitOptions {
                pinned_host: consumer,
                priority: crate::constants::RELEASE_JOB_PRIORITY,
                run_id: stable_run_id("release-delivery", &format!("{}\0{}", run.run_id, d.name)),
                output_uri: run_uri(
                    &run.product,
                    &run.run_id,
                    &format!("deliveries/{}/output", d.name),
                ),
                input_artifacts: resolved.clone(),
                resolved_input_artifacts: resolved,
                secret_env: secret_refs(&d.secret_env),
                ..Default::default()
            };
            let command = delivery_job_command(&run.product).to_string();
            let mut jobs = submit_batch(std::slice::from_ref(&command), &options).await?;
            let job = jobs
                .pop()
                .ok_or_else(|| CmdError::click("durable delivery submission returned no job"))?;
            run.deliveries.insert(
                d.name.clone(),
                DeliveryRun {
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    job_id: job.job_id.clone(),
                    output_prefix: format!("status/{}/output/", job.job_id),
                    required: d.required,
                    state: DeliveryRunState::Submitted,
                    receipt_sha256: None,
                    failure: None,
                },
            );
            save(run).await?;
        }
    }

    // Queue every target before waiting for any one of them. A silent host
    // must not prevent later targets from receiving the same immutable
    // release: they are independent deliveries, even though their required
    // verdicts are collected into one release result.
    for d in &m.deliveries {
        let current = run.deliveries[&d.name].clone();
        if current.state == DeliveryRunState::Passed {
            continue;
        }
        let job = terminal(&store, &current.job_id).await?;
        let ok = matches!(
            job.state.as_str(),
            job_state::COMPLETED | job_state::UPLOADED
        );
        let receipt = store
            .read_bytes(&format!(
                "status/{}/output/delivery-receipt.json",
                current.job_id
            ))
            .await?;
        let updated = run.deliveries.get_mut(&d.name).unwrap();
        updated.receipt_sha256 = receipt.as_deref().map(release_control::sha256_bytes);
        updated.state = if ok {
            DeliveryRunState::Passed
        } else {
            DeliveryRunState::Failed
        };
        updated.failure = if ok {
            None
        } else {
            Some(format!(
                "{}{}",
                job.error.clone().unwrap_or_else(|| job.state.clone()),
                job_output_tail(&store, &current.job_id).await
            ))
        };
        if d.required && !ok {
            return Err(CmdError::click(format!(
                "required delivery {} failed",
                d.name
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RedeliveryStage {
    IntentCreated,
    RunReopened,
    Submitted,
    Terminal,
    RunRestored,
    Completed,
    Failed,
}

impl RedeliveryStage {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeliveryTransaction {
    schema_version: u32,
    retry_token_sha256: String,
    delivery: String,
    previous_run_state: ReleaseRunState,
    pinned_consumer: String,
    request_sha256: String,
    stage: RedeliveryStage,
    job_id: Option<String>,
    receipt_sha256: Option<String>,
    failure: Option<String>,
}

async fn load_redelivery_transaction(
    store: &JobStorage,
    path: &str,
) -> Result<Option<(RedeliveryTransaction, String)>, CmdError> {
    let Some(versioned) = store
        .read_text_versioned(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(Some((
        serde_json::from_str(&versioned.content)?,
        versioned.version,
    )))
}

async fn create_redelivery_transaction(
    store: &JobStorage,
    path: &str,
    transaction: &RedeliveryTransaction,
) -> Result<(), CmdError> {
    let body = serde_json::to_string_pretty(transaction)?;
    if store
        .create_text_if_absent(path, &body)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(());
    }
    Err(CmdError::click(
        "another redelivery transaction won the creation race; retry the command",
    ))
}

async fn replace_redelivery_transaction(
    store: &JobStorage,
    path: &str,
    expected_version: &str,
    transaction: &RedeliveryTransaction,
) -> Result<(), CmdError> {
    store
        .compare_and_swap_text(
            path,
            expected_version,
            &serde_json::to_string_pretty(transaction)?,
        )
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "redelivery transaction changed concurrently; retry the command: {error}"
            ))
        })?;
    Ok(())
}
async fn finish_redelivery(
    args: &ReleaseRedeliverArgs,
    store: &JobStorage,
    transaction_path: &str,
    mut run: ReleaseRun,
    mut transaction: RedeliveryTransaction,
    mut transaction_version: String,
) -> Result<Option<()>, CmdError> {
    if transaction.stage == RedeliveryStage::Terminal {
        if transaction.failure.is_none() {
            let updated = run
                .deliveries
                .get_mut(&transaction.delivery)
                .ok_or_else(|| CmdError::click("release delivery disappeared"))?;
            updated.job_id = transaction
                .job_id
                .clone()
                .ok_or_else(|| CmdError::click("terminal redelivery has no job id"))?;
            updated.output_prefix = format!("status/{}/output/", updated.job_id);
            updated.state = DeliveryRunState::Passed;
            updated.receipt_sha256 = transaction.receipt_sha256.clone();
            updated.failure = None;
        }
        if run.state == ReleaseRunState::Delivering {
            run.state = transaction.previous_run_state.clone();
            save(&mut run).await?;
        } else if run.state != transaction.previous_run_state {
            return Err(CmdError::click(
                "release run changed before redelivery restoration",
            ));
        }
        transaction.stage = RedeliveryStage::RunRestored;
        replace_redelivery_transaction(store, transaction_path, &transaction_version, &transaction)
            .await?;
        (transaction, transaction_version) = load_redelivery_transaction(store, transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }
    if transaction.stage == RedeliveryStage::RunRestored {
        transaction.stage = if transaction.failure.is_some() {
            RedeliveryStage::Failed
        } else {
            RedeliveryStage::Completed
        };
        replace_redelivery_transaction(store, transaction_path, &transaction_version, &transaction)
            .await?;
    }
    if !transaction.stage.terminal() {
        return Ok(None);
    }
    if let Some(failure) = transaction.failure {
        return Err(CmdError::click(format!(
            "redelivery job {} failed: {failure}",
            transaction.job_id.as_deref().unwrap_or("unknown")
        )));
    }
    let job_id = transaction
        .job_id
        .ok_or_else(|| CmdError::click("completed redelivery has no job id"))?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "passed",
                "product": run.product,
                "version": run.version,
                "run_id": run.run_id,
                "delivery": transaction.delivery,
                "job_id": job_id,
                "receipt_sha256": transaction.receipt_sha256,
            }))?
        );
    } else {
        println!(
            "redelivered {} {} run {} delivery {} with job {}",
            run.product, run.version, run.run_id, transaction.delivery, job_id
        );
    }
    Ok(Some(()))
}

/// Re-run one named delivery from an exact completed release run.
///
/// A fixed, separately serialized transaction fences the temporary run-state
/// transition without changing the backwards-compatible `ReleaseRun` schema.
/// Every boundary is durable and the caller's retry token resumes the same job.
pub async fn redeliver(args: &ReleaseRedeliverArgs) -> Result<(), CmdError> {
    if args.retry_token.is_empty() || args.retry_token.len() > 128 {
        return Err(CmdError::click(
            "--retry-token must contain between 1 and 128 bytes",
        ));
    }
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let token_sha = release_control::sha256_bytes(args.retry_token.as_bytes());
    let transaction_path = run_path(&args.product, &args.run_id, "redelivery.json");

    let mut run = load(&args.run_id)
        .await?
        .ok_or_else(|| CmdError::click(format!("release run {} does not exist", args.run_id)))?;
    if run.product != args.product {
        return Err(CmdError::click("release run belongs to another product"));
    }

    let mut loaded = load_redelivery_transaction(&store, &transaction_path).await?;
    if let Some((active, _)) = &loaded {
        if active.retry_token_sha256 != token_sha || active.delivery != args.delivery {
            if !active.stage.terminal() {
                return Err(CmdError::click(format!(
                    "release run has an active redelivery for {}",
                    active.delivery
                )));
            }
            if run.state != active.previous_run_state {
                return Err(CmdError::click(
                    "terminal redelivery transaction has not restored the release run",
                ));
            }
        }
    }
    let matching_transaction = loaded.as_ref().is_some_and(|(active, _)| {
        active.retry_token_sha256 == token_sha && active.delivery == args.delivery
    });
    if matching_transaction {
        let (active, version) = loaded
            .as_ref()
            .cloned()
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
        if finish_redelivery(
            args,
            &store,
            &transaction_path,
            run.clone(),
            active,
            version,
        )
        .await?
        .is_some()
        {
            return Ok(());
        }
    }

    let leaf = format!("deliveries/{}/redeliveries/{token_sha}", args.delivery);
    let request_path = run_path(&run.product, &run.run_id, &format!("{leaf}/request.json"));
    let request_uri = run_uri(&run.product, &run.run_id, &format!("{leaf}/request.json"));
    let (request, request_sha, consumer) = if matching_transaction {
        let active = &loaded
            .as_ref()
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?
            .0;
        let request_bytes = store
            .read_bytes(&request_path)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
            .ok_or_else(|| CmdError::click("redelivery request disappeared"))?;
        if release_control::sha256_bytes(&request_bytes) != active.request_sha256 {
            return Err(CmdError::click("redelivery request digest mismatch"));
        }
        let request: DeliveryRequest = serde_json::from_slice(&request_bytes)?;
        if request.run_id != run.run_id
            || request.product != run.product
            || request.name != args.delivery
        {
            return Err(CmdError::click("redelivery request identity mismatch"));
        }
        (
            request,
            active.request_sha256.clone(),
            active.pinned_consumer.clone(),
        )
    } else {
        let latest = latest_submitted_run(&args.product)
            .await?
            .ok_or_else(|| CmdError::click("product has no submitted release run"))?;
        if latest.run_id != run.run_id
            || latest.source_sha256 != run.source_sha256
            || latest.version != run.version
        {
            return Err(CmdError::click(
                "only the newest exact submitted run may be redelivered",
            ));
        }
        if run.channel != PipelineChannel::Candidate {
            return Err(CmdError::click(
                "redelivery is restricted to completed candidate runs",
            ));
        }
        if run.state != ReleaseRunState::Completed {
            return Err(CmdError::click(
                "a new redelivery requires the latest release run to be completed",
            ));
        }
        let manifest_bytes = store
            .read_bytes(&run_path(&run.product, &run.run_id, "manifest.json"))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
            .ok_or_else(|| CmdError::click("release run manifest is missing"))?;
        if release_control::sha256_bytes(&manifest_bytes) != run.manifest_sha256 {
            return Err(CmdError::click("release run manifest digest mismatch"));
        }
        let manifest = match release_pipeline::parse_product_manifest(&manifest_bytes)
            .map_err(CmdError::click)?
        {
            ProductManifest::Release(manifest) => manifest,
            ProductManifest::NonRelease(_) => {
                return Err(CmdError::click("release run manifest disables releases"))
            }
        };
        let delivery = manifest
            .deliveries
            .iter()
            .find(|delivery| delivery.name == args.delivery)
            .ok_or_else(|| {
                CmdError::click(format!("delivery {:?} is not declared", args.delivery))
            })?;
        let original = run
            .deliveries
            .get(&delivery.name)
            .ok_or_else(|| CmdError::click("release run never completed that delivery"))?;
        if original.state != DeliveryRunState::Passed {
            return Err(CmdError::click(
                "redelivery requires an originally passed delivery",
            ));
        }
        let artifact = super::release_cmd::verified_artifact_for_submit(
            &run.product,
            &run.version,
            &delivery.platform,
        )
        .await?;
        let platform = run
            .platforms
            .get(&delivery.platform)
            .ok_or_else(|| CmdError::click("delivery platform is absent from the release run"))?;
        if platform.state != PlatformRunState::Published
            || platform.artifact_sha256.as_deref() != Some(artifact.artifact_sha256.as_str())
            || platform.release_manifest_sha256.as_deref()
                != Some(artifact.manifest_sha256.as_str())
        {
            return Err(CmdError::click(
                "published artifact no longer matches the release run",
            ));
        }
        let request = DeliveryRequest {
            schema_version: 1,
            run_id: run.run_id.clone(),
            name: delivery.name.clone(),
            product: run.product.clone(),
            version: run.version.clone(),
            platform: delivery.platform.clone(),
            argv: delivery.argv.clone(),
            required: delivery.required,
            secret_env: delivery.secret_env.clone(),
            source_path: "source.tar.gz".into(),
            source_uri: run.source_uri.clone(),
            source_sha256: run.source_sha256.clone(),
            archive_path: "release.tar.gz".into(),
            archive_uri: artifact.archive_uri.clone(),
            archive_sha256: artifact.artifact_sha256.clone(),
            manifest_uri: artifact.manifest_uri.clone(),
            manifest_sha256: artifact.manifest_sha256.clone(),
        };
        let request_bytes = serde_json::to_vec(&request)?;
        let request_sha = release_control::sha256_bytes(&request_bytes);
        queue_immutable(&request_path, &request_bytes).await?;
        let consumer = if delivery.target.is_empty() {
            builder(&manifest.platforms[&delivery.platform].runner_platform)
                .await?
                .1
        } else {
            target_consumer(&delivery.target).await?
        };
        let transaction = RedeliveryTransaction {
            schema_version: 1,
            retry_token_sha256: token_sha.clone(),
            delivery: args.delivery.clone(),
            previous_run_state: run.state.clone(),
            pinned_consumer: consumer.clone(),
            request_sha256: request_sha.clone(),
            stage: RedeliveryStage::IntentCreated,
            job_id: None,
            receipt_sha256: None,
            failure: None,
        };
        match loaded {
            None => create_redelivery_transaction(&store, &transaction_path, &transaction).await?,
            Some((_, version)) => {
                replace_redelivery_transaction(&store, &transaction_path, &version, &transaction)
                    .await?
            }
        }
        loaded = load_redelivery_transaction(&store, &transaction_path).await?;
        (request, request_sha, consumer)
    };
    let mut resolved = Map::new();
    resolved.insert(
        "request".into(),
        input(&request_uri, "delivery-request.json", &request_sha),
    );
    resolved.insert(
        "archive".into(),
        input(
            &request.archive_uri,
            "release.tar.gz",
            &request.archive_sha256,
        ),
    );
    resolved.insert(
        "source".into(),
        input(
            &run_uri(&run.product, &run.run_id, "inputs/source.tar.gz"),
            "source.tar.gz",
            &request.source_sha256,
        ),
    );
    let (mut transaction, mut transaction_version) =
        loaded.ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    let options = SubmitOptions {
        pinned_host: consumer,
        priority: crate::constants::RELEASE_JOB_PRIORITY,
        run_id: stable_run_id(
            "release-redelivery",
            &format!("{}\0{}\0{}", run.run_id, request.name, token_sha),
        ),
        output_uri: run_uri(&run.product, &run.run_id, &format!("{leaf}/output")),
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&request.secret_env),
        ..Default::default()
    };

    if transaction.stage == RedeliveryStage::IntentCreated {
        if run.state == transaction.previous_run_state {
            run.state = ReleaseRunState::Delivering;
            save(&mut run).await?;
        } else if run.state != ReleaseRunState::Delivering {
            return Err(CmdError::click(
                "release run changed before the redelivery fence was installed",
            ));
        }
        transaction.stage = RedeliveryStage::RunReopened;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    if transaction.stage == RedeliveryStage::RunReopened {
        let command = delivery_job_command(&run.product).to_string();
        let job = submit_batch(std::slice::from_ref(&command), &options)
            .await?
            .pop()
            .ok_or_else(|| CmdError::click("durable redelivery submission returned no job"))?;
        transaction.job_id = Some(job.job_id);
        transaction.stage = RedeliveryStage::Submitted;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    if transaction.stage == RedeliveryStage::Submitted {
        let job_id = transaction
            .job_id
            .as_deref()
            .ok_or_else(|| CmdError::click("submitted redelivery has no job id"))?;
        let job = terminal(&store, job_id).await?;
        let ok = matches!(
            job.state.as_str(),
            job_state::COMPLETED | job_state::UPLOADED
        );
        if ok {
            let receipt = store
                .read_bytes(&format!("status/{job_id}/output/delivery-receipt.json"))
                .await?
                .ok_or_else(|| CmdError::click("redelivery produced no delivery receipt"))?;
            transaction.receipt_sha256 = Some(release_control::sha256_bytes(&receipt));
        } else {
            transaction.failure = Some(format!(
                "{}{}",
                job.error.unwrap_or_else(|| job.state.clone()),
                job_output_tail(&store, job_id).await
            ));
        }
        transaction.stage = RedeliveryStage::Terminal;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    finish_redelivery(
        args,
        &store,
        &transaction_path,
        run,
        transaction,
        transaction_version,
    )
    .await?
    .ok_or_else(|| CmdError::click("redelivery stopped before a terminal transaction"))?;
    Ok(())
}

#[derive(Deserialize)]
struct ReplaceRolloutStatus {
    rollout_generation: u64,
    phase: crate::release_agent::RolloutPhase,
    active_version: Option<String>,
    active_sha256: Option<String>,
}

async fn replace_status_exact(
    product: &str,
    target: &str,
    generation: u64,
    version: &str,
    artifact_sha256: &str,
) -> bool {
    let uri = crate::release_agent::release_status_uri(product, target);
    let Ok(bytes) = super::storage::fetch_object(&uri).await else {
        return false;
    };
    let Ok(status) = serde_json::from_slice::<ReplaceRolloutStatus>(&bytes) else {
        return false;
    };
    status.rollout_generation == generation
        && status.phase == crate::release_agent::RolloutPhase::Committed
        && status.active_version.as_deref() == Some(version)
        && status.active_sha256.as_deref() == Some(artifact_sha256)
}

fn replace_service(
    document: &Value,
    logical_service: &str,
    target: &str,
    readiness_path: &str,
) -> Result<(String, String), CmdError> {
    let directory = crate::service_resolution::directory(document)?
        .ok_or_else(|| CmdError::click("service directory disappeared"))?;
    let route = directory
        .services
        .get(logical_service)
        .ok_or_else(|| CmdError::click("release product service disappeared"))?;
    if route.active_host != target {
        return Err(CmdError::click(format!(
            "release product service {logical_service:?} is active on {}, not {target}",
            route.active_host
        )));
    }
    let managed_service = route.managed_service.clone().ok_or_else(|| {
        CmdError::click(format!(
            "release product service {logical_service:?} has no managed service"
        ))
    })?;
    let endpoint = route
        .endpoints
        .get(target)
        .ok_or_else(|| CmdError::click("release product service has no target endpoint"))?;
    let mut readiness = url::Url::parse(&endpoint.url)
        .map_err(|error| CmdError::click(format!("invalid release service endpoint: {error}")))?;
    readiness.set_path(readiness_path);
    readiness.set_query(None);
    readiness.set_fragment(None);
    Ok((managed_service, readiness.to_string()))
}

async fn reconcile(run: &ReleaseRun) -> Result<(), CmdError> {
    // Every poll runs the target's own release agent once, and that run is
    // what advances the rollout state machine. The wait therefore has to
    // cover the phases the agent must pass through, and the last of them is
    // the product's own declared rollback window: the agent leaves
    // Monitoring for Committed only once `rollback_window_seconds` have
    // elapsed since cutover. A fixed ceiling cannot express that. Twenty-four
    // polls five seconds apart gave 120s, while brama, image-video-router and
    // weles-worker each declare a 300s window in the same document read
    // below, so a healthy rollout of any of them reported "did not converge"
    // every single time.
    const POLL_INTERVAL: Duration = Duration::from_secs(5);
    // Headroom for the agent runs themselves and for the poll that observes
    // the commit, which can only be the first one after the window closes.
    const CONVERGENCE_SLACK: Duration = Duration::from_secs(60);

    let (document, _) = super::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("release control disappeared"))?;
    let policy = control
        .products
        .get(&run.product)
        .ok_or_else(|| CmdError::click("release product has no rollout policy"))?;
    let desired = policy
        .desired
        .as_ref()
        .ok_or_else(|| CmdError::click("promoted release has no desired coordinate"))?;
    if desired.version != run.version {
        return Err(CmdError::click(format!(
            "promoted release is {}, not {}",
            desired.version, run.version
        )));
    }
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|e| CmdError::click(e.to_string()))?;
    let runner = crate::deploy::production_runner();
    let mut observed = Vec::new();
    for name in policy.targets.keys() {
        let target = registry
            .targets
            .iter()
            .find(|t| &t.name == name)
            .ok_or_else(|| CmdError::click(format!("rollout target {name} is absent")))?;
        let expected = run
            .platforms
            .get(&policy.targets[name].platform)
            .ok_or_else(|| CmdError::click("rollout platform was not built"))?;
        if policy.strategy.kind == StrategyKind::Replace {
            let artifact_sha256 = expected
                .artifact_sha256
                .as_deref()
                .ok_or_else(|| CmdError::click("rollout artifact digest was not recorded"))?;
            let manifest_sha256 = expected
                .release_manifest_sha256
                .as_deref()
                .ok_or_else(|| CmdError::click("rollout manifest digest was not recorded"))?;
            if !replace_status_exact(
                &run.product,
                name,
                desired.rollout_generation,
                &run.version,
                artifact_sha256,
            )
            .await
            {
                // Same default the validator applies, read through the same
                // constant: a replace target may omit the key, and a submit
                // that refused what validation accepts would be the second
                // reader of one contract disagreeing with the first.
                let readiness_path = policy.targets[name]
                    .readiness_path
                    .as_deref()
                    .unwrap_or(crate::release_control::DEFAULT_REPLACE_READINESS_PATH);
                let (service, readiness_url) =
                    replace_service(&document, &policy.service, name, readiness_path)?;
                super::service::release_pipeline_product(
                    &service,
                    name,
                    &run.product,
                    &run.version,
                    &readiness_url,
                    policy.strategy.readiness_timeout_seconds,
                )
                .await?;
            }
            if !replace_status_exact(
                &run.product,
                name,
                desired.rollout_generation,
                &run.version,
                artifact_sha256,
            )
            .await
            {
                return Err(CmdError::click(format!(
                    "target {name} did not publish committed replace status for {} generation {}",
                    run.version, desired.rollout_generation
                )));
            }
            observed.push(json!({
                "target": name,
                "version": run.version,
                "artifact_sha256": artifact_sha256,
                "manifest_sha256": manifest_sha256
            }));
            continue;
        }
        let script = format!(
            "set -eu\n\
             if [ -x /bin/systemctl ] && /bin/systemctl is-active --quiet wisent-agent.service; then\n\
               environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)\n\
               /usr/bin/env -S \"$environment\" \"$HOME/.stado/bin/stado\" release agent --target {} --product {} --once --json\n\
             else\n\
               \"$HOME/.stado/bin/stado\" release agent --target {} --product {} --once --json\n\
             fi\n",
            crate::deploy::shlex_quote(name),
            crate::deploy::shlex_quote(&run.product),
            crate::deploy::shlex_quote(name),
            crate::deploy::shlex_quote(&run.product)
        );
        let mut last_observation = "product state was not returned".to_string();
        let mut converged = false;
        let budget = Duration::from_secs(
            policy
                .strategy
                .readiness_timeout_seconds
                .saturating_add(policy.strategy.drain_timeout_seconds)
                .saturating_add(policy.strategy.rollback_window_seconds),
        ) + CONVERGENCE_SLACK;
        let deadline = std::time::Instant::now() + budget;
        loop {
            let output = crate::deploy::host_channel::run_script(target, &script, &runner)
                .await
                .map_err(|e| CmdError::click(e.to_string()))?;
            if !output.ok() {
                return Err(CmdError::click(format!(
                    "reconciliation failed on {name}: {}",
                    output.detail()
                )));
            }
            let states: Vec<crate::release_agent::HostReleaseState> =
                serde_json::from_str(output.stdout.trim())?;
            if let Some(state) = states
                .into_iter()
                .find(|state| state.product == run.product)
            {
                if state.rollout_generation > desired.rollout_generation {
                    return Err(CmdError::click(format!(
                        "target {name} advanced to rollout generation {}, beyond {}",
                        state.rollout_generation, desired.rollout_generation
                    )));
                }
                let exact = state.rollout_generation == desired.rollout_generation
                    && state.active.as_ref().is_some_and(|active| {
                        active.version == run.version
                            && Some(active.artifact_sha256.as_str())
                                == expected.artifact_sha256.as_deref()
                            && Some(active.manifest_sha256.as_str())
                                == expected.release_manifest_sha256.as_deref()
                    });
                // An exact active process is still reversible during Monitoring.
                // Record deployment only after the rollout window commits.
                if exact && matches!(state.phase, crate::release_agent::RolloutPhase::Committed) {
                    let active = state.active.as_ref().expect("checked above");
                    observed.push(json!({
                        "target": name,
                        "version": active.version,
                        "artifact_sha256": active.artifact_sha256,
                        "manifest_sha256": active.manifest_sha256
                    }));
                    converged = true;
                    break;
                }
                if state.rollout_generation == desired.rollout_generation
                    && matches!(
                        state.phase,
                        crate::release_agent::RolloutPhase::RolledBack
                            | crate::release_agent::RolloutPhase::Failed
                            | crate::release_agent::RolloutPhase::Quarantined
                    )
                {
                    return Err(CmdError::click(format!(
                        "target {name} refused rollout generation {} in phase {:?}: {}",
                        desired.rollout_generation, state.phase, state.detail
                    )));
                }
                last_observation = format!(
                    "generation={} phase={:?} active={} detail={}",
                    state.rollout_generation,
                    state.phase,
                    state
                        .active
                        .as_ref()
                        .map(|active| active.version.as_str())
                        .unwrap_or("-"),
                    state.detail
                );
            }
            if std::time::Instant::now() + POLL_INTERVAL >= deadline {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if !converged {
            return Err(CmdError::click(format!(
                "target {name} did not converge to {} generation {} within {}s \
                 (readiness {}s + drain {}s + declared rollback window {}s): \
                 {last_observation}",
                run.version,
                desired.rollout_generation,
                budget.as_secs(),
                policy.strategy.readiness_timeout_seconds,
                policy.strategy.drain_timeout_seconds,
                policy.strategy.rollback_window_seconds
            )));
        }
    }
    let receipt = serde_json::to_vec(
        &json!({"schema_version":1,"run_id":run.run_id,"product":run.product,"version":run.version,"targets":observed,"completed_at":Utc::now().to_rfc3339()}),
    )?;
    queue_deployment_receipt(
        &run_path(&run.product, &run.run_id, "deployment.json"),
        &receipt,
    )
    .await
}

/// Resolve one quality/build program the way the agent host actually carries
/// it.
///
/// A LaunchAgent's PATH is minimal by design (`/opt/homebrew/bin:...:/bin`),
/// and the Rust toolchain installs itself into `~/.cargo/bin` — which is how
/// the first stado release job in this fleet's history died: `cargo` existed
/// on the host, the agent's PATH could not see it, and the bare `?` reported
/// `No such file or directory (os error 2)` without naming what it was trying
/// to run. A relative program is looked up here first; a name none of the
/// known homes carries falls through to the spawn's own PATH lookup, so a
/// correctly provisioned PATH keeps working unchanged.
fn resolve_step_program(program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.is_absolute() || program.contains('/') {
        return path.to_path_buf();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        // The owner-only installs first: `stado` itself and the fleet's
        // delivered binaries live here, and the first delivery that ran
        // `stado release install-local` died unable to find the very
        // program that had been delivered to this directory.
        candidates.push(Path::new(&home).join(".stado").join("bin").join(program));
        candidates.push(Path::new(&home).join(".local").join("bin").join(program));
        candidates.push(Path::new(&home).join(".cargo").join("bin").join(program));
    }
    candidates.push(Path::new("/opt/homebrew/bin").join(program));
    candidates.push(Path::new("/usr/local/bin").join(program));
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| path.to_path_buf())
}

fn execute(
    name: &str,
    argv: &[String],
    source: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<StepReceipt, CmdError> {
    let program = resolve_step_program(&argv[0]);
    // The step's start is logged before the spawn, so a step that hangs or
    // dies leaves its name and argv in the job output instead of silence.
    println!(
        "[release-worker] step {name}: {} {}",
        program.display(),
        argv[1..].join(" ")
    );
    let status = Command::new(&program)
        .args(&argv[1..])
        .current_dir(source)
        .envs(environment)
        .status()
        .map_err(|error| {
            CmdError::click(format!(
                "step {name}: cannot run {}: {error}",
                program.display()
            ))
        })?;
    println!("[release-worker] step {name}: exit {:?}", status.code());
    Ok(StepReceipt {
        name: name.into(),
        argv: argv.to_vec(),
        status: if status.success() {
            StepStatus::Passed
        } else {
            StepStatus::Failed
        },
        exit_code: status.code(),
    })
}

/// Install the toolchain components this recipe's gates run, when the recipe
/// is a Rust one and rustup manages the host's toolchain.
///
/// Scoped deliberately: only `cargo fmt` needs `rustfmt` and only
/// `cargo clippy` needs `clippy`, and a recipe that runs neither provisions
/// nothing. rustup reads the toolchain pin from the working directory, so the
/// component lands on exactly the toolchain the gate will use. A host without
/// rustup is left alone — its cargo is not rustup-managed and components are
/// not its concept.
fn ensure_rust_components(
    recipe: &crate::release_pipeline::PlatformRecipe,
    source: &Path,
) -> Result<(), CmdError> {
    let mut needed: Vec<&str> = Vec::new();
    for gate in &recipe.quality {
        let program = gate.argv.first().map(String::as_str).unwrap_or("");
        let subcommand = gate.argv.get(1).map(String::as_str).unwrap_or("");
        if program == "cargo" || program.ends_with("/cargo") {
            match subcommand {
                "fmt" if !needed.contains(&"rustfmt") => needed.push("rustfmt"),
                "clippy" if !needed.contains(&"clippy") => needed.push("clippy"),
                _ => {}
            }
        }
    }
    if needed.is_empty() {
        return Ok(());
    }
    let rustup = resolve_step_program("rustup");
    if !rustup.is_absolute() || !rustup.is_file() {
        println!(
            "[release-worker] no rustup on this host; assuming {} are already provided",
            needed.join(", ")
        );
        return Ok(());
    }
    println!(
        "[release-worker] ensuring toolchain components: {}",
        needed.join(", ")
    );
    let output = Command::new(&rustup)
        .arg("component")
        .arg("add")
        .args(&needed)
        .current_dir(source)
        .output()
        .map_err(|error| CmdError::click(format!("cannot run {}: {error}", rustup.display())))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "rustup component add {} failed: {}",
            needed.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let path = root.join(relative);
    // The recipe's stage map is a declaration about what the build produces, and
    // this is where the two are compared. A bare `?` here reported only
    // `No such file or directory (os error 2)`, so a stage entry whose producer
    // had been deleted looked identical to a broken builder.
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "staged path {} declared by the recipe is not there: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "staged path is a symlink: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        out.push(relative.into());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(CmdError::click("staged path is not regular"));
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        collect(root, &relative.join(entry.file_name()), out)?
    }
    Ok(())
}
fn package(source: &Path, stage: &BTreeMap<String, String>) -> Result<Vec<u8>, CmdError> {
    let gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    let mut archive = tar::Builder::new(gz);
    for (from, to) in stage {
        let base = Path::new(from);
        let mut paths = Vec::new();
        collect(source, base, &mut paths)?;
        for path in paths {
            let suffix = path.strip_prefix(base).unwrap_or(Path::new(""));
            let destination = if suffix.as_os_str().is_empty() {
                PathBuf::from(to)
            } else {
                Path::new(to).join(suffix)
            };
            let bytes = std::fs::read(source.join(&path))?;
            let metadata = std::fs::metadata(source.join(&path))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                header.set_mode(metadata.permissions().mode() & 0o777);
            }
            #[cfg(not(unix))]
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, destination, bytes.as_slice())?
        }
    }
    Ok(archive.into_inner()?.finish()?)
}
fn write_receipt(receipt: &BuildReceipt) -> Result<(), CmdError> {
    std::fs::create_dir_all("output")?;
    std::fs::write("output/receipt.json", serde_json::to_vec(receipt)?)?;
    Ok(())
}

pub async fn worker(args: &ReleaseWorkerArgs) -> Result<(), CmdError> {
    // Name the file. Each of these was a bare `?`, so a missing one surfaced as
    // `Error: No such file or directory (os error 2)` with no path at all, on a
    // builder whose log ended with a successful compile -- and the operator's
    // only recourse was to guess which of four paths it meant.
    let read_named = |path: &dyn AsRef<Path>, what: &str| -> Result<Vec<u8>, CmdError> {
        let path = path.as_ref();
        std::fs::read(path).map_err(|error| {
            CmdError::click(format!("cannot read {what} {}: {error}", path.display()))
        })
    };
    let request_bytes = read_named(&args.request, "the worker request")?;
    let request: WorkerRequest = serde_json::from_slice(&request_bytes)?;
    let manifest_bytes = read_named(&request.manifest_path, "the release manifest")?;
    if request.schema_version != 1
        || release_control::sha256_bytes(&manifest_bytes) != request.manifest_sha256
    {
        return Err(CmdError::click("worker manifest identity mismatch"));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click("worker manifest declares releases:false"));
    };
    if manifest.product != request.product || !manifest.platforms.contains_key(&request.platform) {
        return Err(CmdError::click("worker request disagrees with manifest"));
    }
    let source_bytes = read_named(&request.source_archive, "the source archive")?;
    if release_control::sha256_bytes(&source_bytes) != request.source_sha256 {
        return Err(CmdError::click("worker source digest mismatch"));
    }
    let queue_work_dir = std::env::current_dir()?;
    let temp = tempfile::Builder::new()
        .prefix(".stado-release-worker-")
        .tempdir_in(&queue_work_dir)?;
    let source = temp.path().join("source");
    release_control::safe_extract_archive(&source_bytes, &source).map_err(CmdError::click)?;
    let inputs_root = temp.path().join("inputs");
    std::fs::create_dir_all(&inputs_root)?;
    let mut receipt_inputs = BTreeMap::new();
    for (name, input) in &request.inputs {
        let bytes = read_named(&input.archive_path, &format!("input {name}"))?;
        if release_control::sha256_bytes(&bytes) != input.sha256 {
            return Err(CmdError::click(format!("input {name} digest mismatch")));
        }
        if input.extract {
            release_control::safe_extract_archive(&bytes, &inputs_root.join(&input.mount))
                .map_err(CmdError::click)?;
        } else {
            let destination = inputs_root.join(&input.mount);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(destination, &bytes)?;
        }
        receipt_inputs.insert(
            name.clone(),
            ReceiptInput {
                uri: input.uri.clone(),
                sha256: input.sha256.clone(),
                mount: input.mount.clone(),
                extract: input.extract,
            },
        );
    }
    let output = source.join(".wisent-output");
    std::fs::create_dir_all(&output)?;
    // `WISENT_SOURCE_COMMIT` and `WISENT_SOURCE_SHA256` are the snapshot's own
    // identity, and a build that needs them has nowhere else to get them: the
    // worker unpacks a `git archive`, so there is no repository to ask. Both
    // names are set from the immutable request so a build script cannot inherit
    // an unrelated parent `STADO_SOURCE_REVISION`; Stado's build script requires
    // them to agree exactly. The source commit was already validated before the
    // archive existed.
    let mut environment = BTreeMap::from([
        ("WISENT_SOURCE_DIR".into(), source.display().to_string()),
        ("WISENT_OUTPUT_DIR".into(), output.display().to_string()),
        (
            "WISENT_INPUTS_DIR".into(),
            inputs_root.display().to_string(),
        ),
        ("WISENT_PRODUCT".into(), request.product.clone()),
        ("WISENT_VERSION".into(), request.version.clone()),
        ("WISENT_PLATFORM".into(), request.platform.clone()),
        ("WISENT_SOURCE_COMMIT".into(), request.source_commit.clone()),
        (
            "STADO_SOURCE_REVISION".into(),
            request.source_commit.clone(),
        ),
        ("WISENT_SOURCE_SHA256".into(), request.source_sha256.clone()),
    ]);
    for (name, input) in &request.inputs {
        let key = name
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    b.to_ascii_uppercase() as char
                } else {
                    '_'
                }
            })
            .collect::<String>();
        environment.insert(
            format!("WISENT_INPUT_{key}_DIR"),
            inputs_root.join(&input.mount).display().to_string(),
        );
    }

    // Give the pinned toolchain the components its own gates are about to
    // demand. rustup installs a pinned toolchain on first use WITHOUT optional
    // components, so the first release job on a fresh agent died with
    // "'cargo-fmt' is not installed for the toolchain" — a fact about host
    // provisioning that no release should trip over and no operator should
    // fix by hand, host by host. Adding a component is idempotent and rustup
    // resolves the pin from the working directory, so a provisioned host pays
    // a no-op and a fresh one provisions itself, exactly the way the
    // toolchain itself already arrives.
    ensure_rust_components(&manifest.platforms[&request.platform], &source)?;
    let recipe = &manifest.platforms[&request.platform];
    let job_id = std::env::var("WC_JOB_ID").unwrap_or_default();
    let mut quality = Vec::new();
    for gate in &recipe.quality {
        let step = execute(&gate.name, &gate.argv, &source, &environment)?;
        let passed = step.status == StepStatus::Passed;
        quality.push(step);
        if !passed {
            let receipt = BuildReceipt {
                schema_version: 1,
                run_id: request.run_id.clone(),
                job_id,
                product: request.product,
                version: request.version,
                platform: request.platform,
                builder: request.builder,
                source_commit: request.source_commit,
                source_sha256: request.source_sha256,
                manifest_sha256: request.manifest_sha256,
                inputs: receipt_inputs,
                secret_env: request.secret_env,
                quality,
                build: StepReceipt {
                    name: "build".into(),
                    argv: recipe.build.argv.clone(),
                    status: StepStatus::Failed,
                    exit_code: None,
                },
                status: StepStatus::Failed,
                artifact: None,
                completed_at: Utc::now().to_rfc3339(),
                failure: Some(format!("quality gate {} failed", gate.name)),
            };
            write_receipt(&receipt)?;
            return Err(CmdError::click("release quality gate failed"));
        }
    }
    let build = execute("build", &recipe.build.argv, &source, &environment)?;
    if build.status != StepStatus::Passed {
        let receipt = BuildReceipt {
            schema_version: 1,
            run_id: request.run_id,
            job_id,
            product: request.product,
            version: request.version,
            platform: request.platform,
            builder: request.builder,
            source_commit: request.source_commit,
            source_sha256: request.source_sha256,
            manifest_sha256: request.manifest_sha256,
            inputs: receipt_inputs,
            secret_env: request.secret_env,
            quality,
            build,
            status: StepStatus::Failed,
            artifact: None,
            completed_at: Utc::now().to_rfc3339(),
            failure: Some("build command failed".into()),
        };
        write_receipt(&receipt)?;
        return Err(CmdError::click("release build command failed"));
    }
    // The stage map is relative to `WISENT_OUTPUT_DIR`, which is what every
    // recipe's build script writes into -- brama and skarbiec both install to
    // `$WISENT_OUTPUT_DIR/stage/...`. Packaging resolved it against the source
    // tree instead, so the first entry always reported
    // `staged path .../source/stage/LICENSE ... is not there` and no release
    // carrying a stage mapping could ever be packaged through this path.
    let bytes = package(&output, &recipe.stage)?;
    std::fs::create_dir_all("output")?;
    std::fs::write("output/release.tar.gz", &bytes)?;
    let receipt = BuildReceipt {
        schema_version: 1,
        run_id: request.run_id,
        job_id,
        product: request.product,
        version: request.version,
        platform: request.platform,
        builder: request.builder,
        source_commit: request.source_commit,
        source_sha256: request.source_sha256,
        manifest_sha256: request.manifest_sha256,
        inputs: receipt_inputs,
        secret_env: request.secret_env,
        quality,
        build,
        status: StepStatus::Passed,
        artifact: Some(ArtifactReceipt {
            sha256: release_control::sha256_bytes(&bytes),
            bytes: bytes.len() as u64,
            path: "release.tar.gz".into(),
        }),
        completed_at: Utc::now().to_rfc3339(),
        failure: None,
    };
    write_receipt(&receipt)
}

async fn require_current_delivery(request: &DeliveryRequest) -> Result<(), CmdError> {
    let latest = latest_submitted_run(&request.product)
        .await?
        .ok_or_else(|| {
            CmdError::click(format!(
                "delivery names no submitted release run for {}",
                request.product
            ))
        })?;
    let latest_exact = latest.run_id == request.run_id
        && latest.version == request.version
        && latest.source_sha256 == request.source_sha256
        && latest.source_uri == request.source_uri;
    if !latest_exact {
        return Err(CmdError::click(format!(
            "delivery for {} {} source {} was superseded by release run {} source {}; refusing \
             the stale coordinate",
            request.product,
            request.version,
            request.source_sha256,
            latest.run_id,
            latest.source_sha256
        )));
    }
    let run = load(&request.run_id).await?.ok_or_else(|| {
        CmdError::click(format!(
            "delivery names missing release run {}",
            request.run_id
        ))
    })?;
    let platform = run.platforms.get(&request.platform);
    let exact = run.state == ReleaseRunState::Delivering
        && run.product == request.product
        && run.version == request.version
        && run.source_sha256 == request.source_sha256
        && run.source_uri == request.source_uri
        && platform.is_some_and(|platform| {
            platform.state == PlatformRunState::Published
                && platform.artifact_sha256.as_deref() == Some(request.archive_sha256.as_str())
                && platform.release_manifest_sha256.as_deref()
                    == Some(request.manifest_sha256.as_str())
        });
    if !exact {
        return Err(CmdError::click(format!(
            "delivery {} {} {} does not match its current published run; refusing the stale \
             coordinate",
            request.product, request.version, request.platform
        )));
    }
    Ok(())
}

pub async fn delivery_worker(args: &DeliveryWorkerArgs) -> Result<(), CmdError> {
    let request: DeliveryRequest = serde_json::from_slice(&std::fs::read(&args.request)?)?;
    require_current_delivery(&request).await?;
    let archive = std::fs::read(&request.archive_path)?;
    let source_archive = std::fs::read(&request.source_path)?;
    if request.schema_version != 1
        || release_control::sha256_bytes(&archive) != request.archive_sha256
        || release_control::sha256_bytes(&source_archive) != request.source_sha256
    {
        return Err(CmdError::click("delivery input identity mismatch"));
    }
    let source_root = std::env::current_dir()?.join("delivery-source");
    release_control::safe_extract_archive(&source_archive, &source_root)
        .map_err(CmdError::click)?;
    let output = std::env::current_dir()?.join("output");
    std::fs::create_dir_all(&output)?;
    let environment = BTreeMap::from([
        (
            "WISENT_SOURCE_DIR".into(),
            source_root.display().to_string(),
        ),
        (
            "WISENT_RELEASE_ARCHIVE".into(),
            std::fs::canonicalize(&request.archive_path)?
                .display()
                .to_string(),
        ),
        ("WISENT_RELEASE_URI".into(), request.archive_uri.clone()),
        (
            "WISENT_RELEASE_SHA256".into(),
            request.archive_sha256.clone(),
        ),
        (
            "WISENT_RELEASE_MANIFEST_URI".into(),
            request.manifest_uri.clone(),
        ),
        (
            "WISENT_RELEASE_MANIFEST_SHA256".into(),
            request.manifest_sha256.clone(),
        ),
        ("WISENT_PRODUCT".into(), request.product.clone()),
        ("WISENT_VERSION".into(), request.version.clone()),
        ("WISENT_PLATFORM".into(), request.platform.clone()),
        ("WISENT_OUTPUT_DIR".into(), output.display().to_string()),
    ]);
    let mut delivery_argv = request.argv.clone();
    if request.product == "stado" && delivery_argv.first().is_some_and(|value| value == "stado") {
        delivery_argv[0] = std::env::current_exe()?.display().to_string();
    }
    let step = execute(&request.name, &delivery_argv, &source_root, &environment)?;
    let receipt = DeliveryReceipt {
        schema_version: 1,
        run_id: request.run_id,
        job_id: std::env::var("WC_JOB_ID").unwrap_or_default(),
        name: request.name,
        product: request.product,
        version: request.version,
        platform: request.platform,
        argv: delivery_argv,
        required: request.required,
        secret_env: request.secret_env,
        archive_uri: request.archive_uri,
        archive_sha256: request.archive_sha256,
        manifest_uri: request.manifest_uri,
        manifest_sha256: request.manifest_sha256,
        status: step.status.clone(),
        exit_code: step.exit_code,
        completed_at: Utc::now().to_rfc3339(),
    };
    std::fs::write(
        output.join("delivery-receipt.json"),
        serde_json::to_vec(&receipt)?,
    )?;
    if step.status == StepStatus::Passed {
        Ok(())
    } else {
        Err(CmdError::click("release delivery failed"))
    }
}
