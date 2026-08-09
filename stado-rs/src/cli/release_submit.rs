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
fn run_uri(id: &str, leaf: &str) -> String {
    format!("stado://release-runs/{id}/{leaf}")
}
async fn save(run: &mut ReleaseRun) -> Result<(), CmdError> {
    run.updated_at = Utc::now().to_rfc3339();
    let uri = run_uri(&run.run_id, "run.json");
    let b = serde_json::to_vec(run)?;
    if let Some((_, v)) = super::storage::fetch_object_versioned(&uri).await? {
        super::storage::compare_and_swap_object(&uri, &b, "application/json", &v).await
    } else {
        let f = tempfile::NamedTempFile::new()?;
        std::fs::write(f.path(), &b)?;
        super::storage::store_object(
            &uri,
            &f.path().display().to_string(),
            "application/json",
            true,
        )
        .await
        .map(|_| ())
    }
}
async fn load(id: &str) -> Result<Option<ReleaseRun>, CmdError> {
    match super::storage::fetch_object(&run_uri(id, "run.json")).await {
        Ok(v) => Ok(Some(serde_json::from_slice(&v)?)),
        Err(_) => Ok(None),
    }
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
async fn builder(platform: &str) -> Result<crate::targets::ComputeTarget, CmdError> {
    let r = crate::targets::fetch_registry_remote()
        .await
        .map_err(|e| CmdError::click(e.to_string()))?;
    let mut c: Vec<_> = r
        .targets
        .into_iter()
        .filter(|t| t.release_platform == platform)
        .collect();
    c.sort_by(|a, b| a.name.cmp(&b.name));
    c.into_iter().next().ok_or_else(|| {
        CmdError::click(format!(
            "no registry builder has verified release_platform {platform}"
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
    let host = builder(&recipe.runner_platform).await?;
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
        resolved.insert(format!("input-{name}"), input(&v.uri, &path, &v.sha256));
        inputs.insert(
            name.clone(),
            WorkerInput {
                uri: v.uri.clone(),
                sha256: v.sha256.clone(),
                archive_path: path,
                mount: v.mount.clone(),
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
    let uri = run_uri(id, &format!("requests/{platform}.json"));
    immutable(&uri, &bytes, "application/json", &BTreeMap::new()).await?;
    resolved.insert("request".into(), input(&uri, "release-request.json", &sha));
    let options = SubmitOptions {
        pinned_host: host.name.clone(),
        run_id: id.into(),
        output_uri: run_uri(id, &format!("platforms/{platform}/output")),
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&recipe.secret_env),
        ..Default::default()
    };
    let job = submit_job(
        "stado release worker --request release-request.json",
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
    let encoded = crate::credential_store::read_string(&item, "private_key_pkcs8_base64")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} field private_key_pkcs8_base64 is required"
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
        return Err(CmdError::click(format!(
            "release job {} failed: {}",
            rec.job_id,
            job.error.unwrap_or(job.state)
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
    let manifest_uri = run_uri(&id, "manifest.json");
    immutable(
        &manifest_uri,
        &manifest_bytes,
        "application/json",
        &BTreeMap::new(),
    )
    .await?;
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
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    for p in &platforms {
        if !run.platforms.contains_key(p) {
            let r = enqueue(
                &id,
                &m,
                &args.version,
                p,
                &commit,
                &source_sha,
                &source_uri,
                &manifest_sha,
                &manifest_uri,
            )
            .await?;
            run.platforms.insert(p.clone(), r);
            save(&mut run).await?
        }
    }
    run.state = ReleaseRunState::Waiting;
    save(&mut run).await?;
    let store = JobStorage::new().await?;
    let (key, private) = signing(&run.product).await?;
    run.state = ReleaseRunState::Publishing;
    save(&mut run).await?;
    let mut artifacts = BTreeMap::new();
    for p in &platforms {
        let a = if run.platforms[p].state == PlatformRunState::Published {
            super::release_cmd::verified_artifact_for_submit(&run.product, &run.version, p).await?
        } else {
            let a = publish(&mut run, &m, p, &store, &key, &private).await?;
            save(&mut run).await?;
            a
        };
        artifacts.insert(p.clone(), a);
    }
    run.state = ReleaseRunState::Delivering;
    save(&mut run).await?;
    run_deliveries(&mut run, &m, &artifacts).await?;
    if m.promotion.reconcile {
        super::release_cmd::promote_for_submit(&run.product, &run.version, run.channel).await?;
        reconcile(&run).await?;
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
            let uri = run_uri(&run.run_id, &format!("deliveries/{}/request.json", d.name));
            immutable(&uri, &bytes, "application/json", &BTreeMap::new()).await?;
            let mut resolved = Map::new();
            resolved.insert("request".into(), input(&uri, "delivery-request.json", &sha));
            resolved.insert(
                "archive".into(),
                input(&a.archive_uri, "release.tar.gz", &a.artifact_sha256),
            );
            resolved.insert(
                "source".into(),
                input(&run.source_uri, "source.tar.gz", &run.source_sha256),
            );
            let host = builder(&m.platforms[&d.platform].runner_platform).await?;
            let options = SubmitOptions {
                pinned_host: host.name,
                run_id: run.run_id.clone(),
                output_uri: run_uri(&run.run_id, &format!("deliveries/{}/output", d.name)),
                input_artifacts: resolved.clone(),
                resolved_input_artifacts: resolved,
                secret_env: secret_refs(&d.secret_env),
                ..Default::default()
            };
            let job = submit_job(
                "stado release delivery-worker --request delivery-request.json",
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
            "set -eu\nstado release agent --target {} --once --json\n",
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
    immutable(
        &run_uri(&run.run_id, "deployment.json"),
        &receipt,
        "application/json",
        &BTreeMap::new(),
    )
    .await
}

fn execute(
    name: &str,
    argv: &[String],
    source: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<StepReceipt, CmdError> {
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(source)
        .envs(environment)
        .status()?;
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
fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let path = root.join(relative);
    let metadata = std::fs::symlink_metadata(&path)?;
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
    let request: WorkerRequest = serde_json::from_slice(&std::fs::read(&args.request)?)?;
    let manifest_bytes = std::fs::read(&request.manifest_path)?;
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
    let source_bytes = std::fs::read(&request.source_archive)?;
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
        let bytes = std::fs::read(&input.archive_path)?;
        if release_control::sha256_bytes(&bytes) != input.sha256 {
            return Err(CmdError::click(format!("input {name} digest mismatch")));
        }
        release_control::safe_extract_archive(&bytes, &inputs_root.join(&input.mount))
            .map_err(CmdError::click)?;
        receipt_inputs.insert(
            name.clone(),
            ReceiptInput {
                uri: input.uri.clone(),
                sha256: input.sha256.clone(),
                mount: input.mount.clone(),
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
    let bytes = package(&source, &recipe.stage)?;
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
