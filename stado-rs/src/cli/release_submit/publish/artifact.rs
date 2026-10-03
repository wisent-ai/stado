//! Publish one platform's qualified build onto its immutable release
//! coordinate.

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::jobs::terminal::{job_output_tail, terminal};
use crate::cli::storage;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control::{self, QualificationStatus, ReleaseArtifactRef, ReleaseQualification};
use crate::release_pipeline::{
    self, BuildReceipt, PlatformRunState, ReleasePipelineManifest, ReleaseRun, StepStatus,
};

pub(crate) async fn publish(
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
    // The receipt names the build the job belonged to, which is the run's
    // build; a run of its own never queued this job.
    let build_id = run.build_id.as_deref().unwrap_or(&run.run_id);
    if r.run_id != build_id
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
    let (a, _) = release_cmd::publish_pipeline_release(release_cmd::PipelinePublishRequest {
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
    readable(run, p).await?;
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

/// A coordinate counts as published only once a reader can see it. The
/// deliveries queued next fetch `release.tar.gz` on their own hosts and
/// refuse a coordinate whose objects they cannot read, so a run that marked
/// the platform Published on the writer's word alone failed its required
/// delivery instead. The commit marker (`release.json`) and the archive are
/// read back through the same release reader `reconcile_published` uses;
/// when either is missing the platform is refused here, before any delivery
/// is queued, naming the objects and the origin, and the next pass or
/// `stado release resume` finds the coordinate through `reconcile_published`
/// once it is visible.
async fn readable(run: &ReleaseRun, p: &str) -> Result<(), CmdError> {
    let base =
        release_control::release_base(&run.product, &run.version, p).map_err(CmdError::click)?;
    let mut unseen = Vec::new();
    for name in [
        release_control::RELEASE_MANIFEST_NAME,
        release_control::RELEASE_ARCHIVE_NAME,
    ] {
        let uri = format!("{base}/{name}");
        if !storage::release_object_present(&uri).await? {
            unseen.push(uri);
        }
    }
    if unseen.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} {} {p} was written, but {} cannot read {}; no delivery was queued. \
         `stado release resume {}` marks it published once the coordinate is readable there",
        run.product,
        run.version,
        storage::release_reader_origin()?,
        unseen.join(" and "),
        run.run_id,
    )))
}
