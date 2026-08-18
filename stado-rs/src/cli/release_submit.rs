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
use crate::queue::submit::{submit_job, SubmitOptions};
use crate::release_control::{self, QualificationStatus, ReleaseArtifactRef, ReleaseQualification};
use crate::release_pipeline::{
    self, ArtifactReceipt, BuildReceipt, CatalogSourceIdentity, DeliveryRun, DeliveryRunState,
    PipelineChannel, PlatformRun, PlatformRunState, ProductManifest, ReceiptInput,
    ReleasePipelineManifest, ReleaseRun, ReleaseRunState, StepReceipt, StepStatus, WorkerInput,
    WorkerRequest, PRODUCT_MANIFEST,
};

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
    match super::storage::fetch_object(uri).await {
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
    let mut live_consumers = BTreeMap::new();
    for consumer in capacity.keys() {
        let identity = consumer.strip_prefix("local-").unwrap_or(consumer);
        if let Some(target) = registry
            .lookup_self(identity)
            .map_err(|error| CmdError::click(error.to_string()))?
        {
            live_consumers
                .entry(target.name.clone())
                .or_insert_with(|| consumer.clone());
        }
    }
    let declared_for_platform = registry
        .targets
        .iter()
        .filter(|target| target.release_platform == platform)
        .count();
    let mut candidates: Vec<_> = registry
        .targets
        .into_iter()
        .filter_map(|target| {
            if target.release_platform != platform {
                return None;
            }
            let consumer = live_consumers.get(&target.name)?.clone();
            Some((target, consumer))
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
        CmdError::click(format!(
            "no live fleet builder is broadcasting verified release_platform \
             {platform}; capacity read from {store} namespace {:?} listed {} live \
             consumer(s) and the registry declares {} target(s) for that platform",
            crate::config::wc_stado_storage_namespace(),
            live_consumers.len(),
            declared_for_platform,
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

// The build request's identity: every argument is a distinct coordinate the
// worker is required to receive, and each is already validated by the caller.
#[allow(clippy::too_many_arguments)]
async fn enqueue(
    id: &str,
    m: &ReleasePipelineManifest,
    version: &str,
    platform: &str,
    commit: &str,
    source_sha: &str,
    source_uri: &str,
    manifest_sha: &str,
    manifest_uri: &str,
) -> Result<PlatformRun, CmdError> {
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
    let options = SubmitOptions {
        pinned_host: consumer,
        run_id: id.into(),
        output_uri: run_uri(&m.product, id, &format!("platforms/{platform}/output")),
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&recipe.secret_env),
        ..Default::default()
    };
    let job = submit_job(
        "$HOME/.stado/bin/stado release worker --request release-request.json",
        &options,
    )
    .await?;
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
    let runtime = m.runtime.as_ref();
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
    let (commit, archive) = snapshot(&root)?;
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
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    for p in &platforms {
        if !run.platforms.contains_key(p) || run.platforms[p].state == PlatformRunState::Failed {
            let r = match enqueue(
                &id,
                &m,
                &args.version,
                p,
                &commit,
                &source_sha,
                &source_input_uri,
                &manifest_sha,
                &manifest_uri,
            )
            .await
            {
                Ok(run) => run,
                Err(error) => return Err(persist_failure(&mut run, error).await),
            };
            run.platforms.insert(p.clone(), r);
            save(&mut run).await?
        }
    }
    run.state = ReleaseRunState::Waiting;
    save(&mut run).await?;
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_failure(&mut run, CmdError::click(error.to_string())).await)
        }
    };
    let (key, private) = match signing(&run.product).await {
        Ok(signing) => signing,
        Err(error) => return Err(persist_failure(&mut run, error).await),
    };
    run.state = ReleaseRunState::Publishing;
    save(&mut run).await?;
    let mut artifacts = BTreeMap::new();
    for p in &platforms {
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
                return Err(persist_failure(&mut run, error).await);
            }
        };
        save(&mut run).await?;
        artifacts.insert(p.clone(), a);
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

async fn run_deliveries(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    for d in &m.deliveries {
        if !run.deliveries.contains_key(&d.name) {
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
            let (_, consumer) = builder(&m.platforms[&d.platform].runner_platform).await?;
            let options = SubmitOptions {
                pinned_host: consumer,
                run_id: run.run_id.clone(),
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
            let job = submit_job(
                "$HOME/.stado/bin/stado release delivery-worker --request delivery-request.json",
                &options,
            )
            .await?;
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
        let current = run.deliveries[&d.name].clone();
        if current.state != DeliveryRunState::Submitted {
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
            Some(job.error.unwrap_or(job.state))
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

async fn reconcile(run: &ReleaseRun) -> Result<(), CmdError> {
    let (document, _) = super::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("release control disappeared"))?;
    let policy = control
        .products
        .get(&run.product)
        .ok_or_else(|| CmdError::click("release product has no rollout policy"))?;
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
        let script = format!(
            "set -eu\n\
             if [ -x /bin/systemctl ] && /bin/systemctl is-active --quiet wisent-agent.service; then\n\
               environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)\n\
               /usr/bin/env -S \"$environment\" \"$HOME/.stado/bin/stado\" release agent --target {} --once --json\n\
             else\n\
               \"$HOME/.stado/bin/stado\" release agent --target {} --once --json\n\
             fi\n",
            crate::deploy::shlex_quote(name),
            crate::deploy::shlex_quote(name)
        );
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
        let state = states
            .into_iter()
            .find(|s| s.product == run.product)
            .ok_or_else(|| CmdError::click(format!("target {name} omitted release state")))?;
        let active = state
            .active
            .ok_or_else(|| CmdError::click(format!("target {name} has no active release")))?;
        let expected = run
            .platforms
            .get(&policy.targets[name].platform)
            .ok_or_else(|| CmdError::click("rollout platform was not built"))?;
        if active.version != run.version
            || Some(active.artifact_sha256.as_str()) != expected.artifact_sha256.as_deref()
            || Some(active.manifest_sha256.as_str()) != expected.release_manifest_sha256.as_deref()
        {
            return Err(CmdError::click(format!(
                "target {name} did not report exact promoted digests"
            )));
        }
        observed.push(json!({"target":name,"version":active.version,"artifact_sha256":active.artifact_sha256,"manifest_sha256":active.manifest_sha256}));
    }
    let receipt = serde_json::to_vec(
        &json!({"schema_version":1,"run_id":run.run_id,"product":run.product,"version":run.version,"targets":observed,"completed_at":Utc::now().to_rfc3339()}),
    )?;
    queue_immutable(
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
    let temp = tempfile::tempdir()?;
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

pub async fn delivery_worker(args: &DeliveryWorkerArgs) -> Result<(), CmdError> {
    let request: DeliveryRequest = serde_json::from_slice(&std::fs::read(&args.request)?)?;
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
    let step = execute(&request.name, &request.argv, &source_root, &environment)?;
    let receipt = DeliveryReceipt {
        schema_version: 1,
        run_id: request.run_id,
        job_id: std::env::var("WC_JOB_ID").unwrap_or_default(),
        name: request.name,
        product: request.product,
        version: request.version,
        platform: request.platform,
        argv: request.argv,
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
