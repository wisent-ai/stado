//! Admission of one release: the build it is made from, and the run object
//! that records it before any job is read.
//!
//! `--source` builds the checkout first, through the same records `stado
//! build submit` keeps, and `--build` takes a build that has already passed.
//! Either way the run names the build it consumes.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::cli::build_cmd::{
    current_build, ensure_object_store, read_source, record_build, stage_source,
};
use crate::cli::release_cmd;
use crate::cli::release_submit::publish::signing::require_rollback_compatibility;
use crate::cli::release_submit::run::source::{build_path, identity};
use crate::cli::release_submit::run::state::load;
use crate::cli::release_submit::ReleaseSubmitArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{
    self, BuildRun, BuildRunState, PipelineChannel, ProductManifest, ReleasePipelineManifest,
    ReleaseRun, ReleaseRunState,
};

use super::continue_run;

pub async fn submit(args: &ReleaseSubmitArgs) -> Result<(), CmdError> {
    let channel = args.channel.into();
    let (build, m, bound_checkout) = match (&args.source, &args.build) {
        (Some(source), None) => {
            let version = args
                .version
                .as_deref()
                .ok_or_else(|| CmdError::usage("--version is required with --source"))?;
            let reading = read_source(source, args.commit.as_deref(), version)?;
            let m = reading.manifest.clone();
            require_channel(&m, channel)?;
            require_rollback_compatibility(&m, version).await?;
            ensure_object_store().await?;
            claim_platforms(&m, version, &reading.commit).await?;
            let staged = stage_source(&reading).await?;
            // Recorded, not queued: the run's own continuation queues what
            // the build owes, so a refused builder is written on the run the
            // CLI and Desktop read, exactly once.
            let build = record_build(&reading, &staged, version).await?;
            (build, m, Some((reading.root, reading.commit)))
        }
        (None, Some(build_id)) => {
            let build = current_build(build_id, false).await?;
            if build.state != BuildRunState::Passed {
                return Err(CmdError::click(format!(
                    "build {build_id} is {}, not passed: a release consumes only a passed build; `stado build status {build_id}` shows what its platforms did",
                    build.state.word()
                )));
            }
            let m = build_manifest(&build).await?;
            require_channel(&m, channel)?;
            require_rollback_compatibility(&m, &build.version).await?;
            ensure_object_store().await?;
            claim_platforms(&m, &build.version, &build.source_commit).await?;
            (build, m, None)
        }
        _ => return Err(CmdError::usage("release submit needs --source or --build")),
    };
    let id = identity(
        &m.product,
        &build.version,
        channel,
        &build.source_sha256,
        &build.manifest_sha256,
    );
    let now = Utc::now().to_rfc3339();
    let mut run = load(&id).await?.unwrap_or(ReleaseRun {
        schema_version: 1,
        run_id: id.clone(),
        product: m.product.clone(),
        version: build.version.clone(),
        channel,
        source_commit: build.source_commit.clone(),
        source_sha256: build.source_sha256.clone(),
        source_uri: build.source_uri.clone(),
        manifest_sha256: build.manifest_sha256.clone(),
        manifest_uri: build.manifest_uri.clone(),
        build_id: Some(build.build_id.clone()),
        state: ReleaseRunState::Submitting,
        platforms: BTreeMap::new(),
        deliveries: BTreeMap::new(),
        failure: None,
        created_at: now.clone(),
        updated_at: now,
    });
    if run.source_commit != build.source_commit
        || run.source_sha256 != build.source_sha256
        || run.manifest_sha256 != build.manifest_sha256
    {
        return Err(CmdError::click("durable release run identity mismatch"));
    }
    // A run recorded before builds had records of their own adopts this
    // build: the jobs it queued under itself are not read again, the
    // build's are.
    if run.build_id.is_none() {
        run.build_id = Some(build.build_id.clone());
        run.manifest_uri = build.manifest_uri.clone();
    }
    if run.state == ReleaseRunState::Submitting {
        if let Some((root, commit)) = &bound_checkout {
            crate::cli::release_submit::changes::bind(root, commit, &id, &m.product).await?;
        }
    }
    // Submitting is queueing. The builds run in the fleet, and the control
    // host's release agent signs, publishes and delivers when they are done;
    // the operator's terminal is not the place to wait an hour for a builder.
    continue_run(run, m, args.json, false).await
}

fn require_channel(m: &ReleasePipelineManifest, channel: PipelineChannel) -> Result<(), CmdError> {
    if !m.promotion.channels.contains(&channel) {
        return Err(CmdError::click(
            "requested channel is forbidden by promotion policy",
        ));
    }
    Ok(())
}

/// Reserve every platform coordinate before this submission can become the
/// newest durable run. Delivery workers fence themselves against that
/// newest run. When the claim lived only in `publish`, a second source tree
/// could persist a newer run for the same version, fail later against the
/// first tree's immutable claim, and still make every valid delivery from
/// the first run refuse itself as superseded. Claiming in the manifest's
/// stable platform order makes that incompatible submission fail before it
/// can become a delivery fence.
async fn claim_platforms(
    m: &ReleasePipelineManifest,
    version: &str,
    commit: &str,
) -> Result<(), CmdError> {
    for platform in m.platforms.keys() {
        release_cmd::claim_release_coordinate(&m.product, version, platform, commit).await?;
    }
    Ok(())
}

/// The manifest a build was made from, read back from the build's own
/// staged copy and checked against the digest the build records.
async fn build_manifest(build: &BuildRun) -> Result<ReleasePipelineManifest, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let path = build_path(&build.product, &build.build_id, "manifest.json");
    let bytes = store
        .read_bytes(&path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click(format!("build manifest is missing: {path}")))?;
    if release_control::sha256_bytes(&bytes) != build.manifest_sha256 {
        return Err(CmdError::click("build manifest digest mismatch"));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click("build manifest disables releases"));
    };
    if manifest.product != build.product {
        return Err(CmdError::click(
            "recorded build manifest disagrees with the build",
        ));
    }
    Ok(manifest)
}
