//! A staged source's build: recorded once, published and queued, the part
//! of `stado build submit` a release submission and `build newest` share.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::cli::release_submit::{
    build_identity, build_path, build_uri, enqueue_platforms, load_build, persist_build_failure,
    queue_immutable, refresh_build, save_build,
};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{self, BuildRun, BuildRunState, ReleasePipelineManifest};

use super::submit::{publish_source, SourceReading, StagedSource};

/// The record of this staged source's build: created if it is new, loaded
/// if it is not, its inputs staged where its jobs read them. Nothing is
/// queued here; `queue_build` does that, and a release run does it through
/// its own continuation.
pub(crate) async fn record_build(
    reading: &SourceReading,
    staged: &StagedSource,
    version: &str,
) -> Result<BuildRun, CmdError> {
    let m = &reading.manifest;
    let id = build_identity(
        &m.product,
        version,
        &staged.source_sha256,
        &staged.manifest_sha256,
    );
    {
        let _phase = super::timing::phase("stage the build inputs in the queue");
        queue_immutable(
            &build_path(&m.product, &id, "inputs/source.tar.gz"),
            &staged.archive,
        )
        .await?;
        queue_immutable(
            &build_path(&m.product, &id, "manifest.json"),
            &reading.manifest_bytes,
        )
        .await?;
    }
    let now = Utc::now().to_rfc3339();
    let mut build = load_build(&id).await?.unwrap_or(BuildRun {
        schema_version: 1,
        build_id: id.clone(),
        product: m.product.clone(),
        version: version.to_owned(),
        source_commit: reading.commit.clone(),
        source_sha256: staged.source_sha256.clone(),
        source_uri: staged.source_uri.clone(),
        manifest_sha256: staged.manifest_sha256.clone(),
        manifest_uri: build_uri(&m.product, &id, "manifest.json"),
        state: BuildRunState::Waiting,
        platforms: BTreeMap::new(),
        failure: None,
        created_at: now.clone(),
        updated_at: now,
    });
    if build.source_commit != reading.commit
        || build.source_sha256 != staged.source_sha256
        || build.manifest_sha256 != staged.manifest_sha256
    {
        return Err(CmdError::refused("durable build identity mismatch"));
    }
    {
        let _phase = super::timing::phase("bind the commit to its release batch");
        crate::cli::release_submit::changes::bind(&reading.root, &reading.commit, &id, &m.product)
            .await?;
    }
    build.failure = None;
    save_build(&mut build).await?;
    Ok(build)
}

/// Queue every platform the build still owes and read what its jobs did.
/// The enqueue failure that stopped the walk, if one did, comes back so the
/// caller can say what was queued before reporting it; a build that queued
/// nothing records that failure as its own and it is returned as the error.
pub(crate) async fn queue_build(
    build: &mut BuildRun,
    m: &ReleasePipelineManifest,
) -> Result<Option<CmdError>, CmdError> {
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_build_failure(build, CmdError::click(error.to_string())).await)
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    let enqueue_phase = super::timing::phase("queue the platform jobs");
    let mut enqueue_failure = enqueue_platforms(&store, build, m, &platforms).await?;
    drop(enqueue_phase);
    let queued_nothing = build
        .platforms
        .values()
        .all(|platform| platform.state == release_pipeline::PlatformRunState::Failed);
    if queued_nothing {
        if let Some(error) = enqueue_failure.take() {
            return Err(persist_build_failure(build, error).await);
        }
    }
    let _phase = super::timing::phase("read what the queued jobs did");
    refresh_build(&store, build, m).await?;
    if let Some(error) = &enqueue_failure {
        build.failure = Some(format!("not every platform was queued: {error}"));
    }
    save_build(build).await?;
    Ok(enqueue_failure)
}

/// The build of this snapshot, recorded, published and queued.
///
/// The build is recorded and bound to the pushed changes it covers before
/// the product is enrolled and its source published, so a refusal there is
/// written on the build and those changes read `failed` instead of waiting
/// for a build that was never recorded. Enrollment used to run first, and a
/// submission it refused left every covered change `queued` for good.
pub(crate) async fn ensure_build(
    reading: &SourceReading,
    staged: &StagedSource,
    version: &str,
) -> Result<(BuildRun, Option<CmdError>), CmdError> {
    let mut build = record_build(reading, staged, version).await?;
    if let Err(error) = publish_source(reading, staged).await {
        return Err(persist_build_failure(&mut build, error).await);
    }
    let enqueue_failure = queue_build(&mut build, &reading.manifest).await?;
    cancel_superseded(&build, &reading.root).await;
    Ok((build, enqueue_failure))
}

/// Cancel the jobs of this product's older builds that are still running on
/// a platform this build also queued, when this build's commit contains
/// theirs, and say each one on stderr.
///
/// Without this an older build keeps compiling after a build of a descendant
/// commit is queued, holding the Cargo build-directory lock the new build
/// waits on: every minute of it is spent on a result nobody will use. A build of a
/// commit that is not an ancestor of this one, or of the same commit, is
/// left alone. A cancellation or a read that fails is named and the new
/// build stands: it is queued either way, and only the wait is lost.
async fn cancel_superseded(build: &BuildRun, root: &std::path::Path) {
    let older = match super::report::recent_builds(Some(&build.product), None).await {
        Ok(builds) => builds,
        Err(error) => {
            eprintln!(
                "build {}: older {} builds could not be read, so none was cancelled: {error}",
                build.build_id, build.product
            );
            return;
        }
    };
    let mut facade = None;
    for old in older.iter().filter(|old| {
        old.build_id != build.build_id
            && old.state == release_pipeline::BuildRunState::Waiting
            && old.source_commit != build.source_commit
    }) {
        match crate::cli::release_submit::changes::contains(
            root,
            &old.source_commit,
            &build.source_commit,
        ) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                eprintln!(
                    "build {}: whether {} contains build {}'s {} is unknown, so it was not cancelled: {error}",
                    build.build_id, build.source_commit, old.build_id, old.source_commit
                );
                continue;
            }
        }
        for platform in old.platforms.values().filter(|platform| {
            platform.state == release_pipeline::PlatformRunState::Submitted
                && build.platforms.contains_key(&platform.platform)
        }) {
            if facade.is_none() {
                match crate::machine::MachineFacade::new().await {
                    Ok(opened) => facade = Some(opened),
                    Err(error) => {
                        eprintln!(
                            "build {}: the queue could not be opened, so superseded builds were not cancelled: {error}",
                            build.build_id
                        );
                        return;
                    }
                }
            }
            let Some(queue) = facade.as_ref() else {
                return;
            };
            match queue.cancel_job(&platform.job_id).await {
                Ok(_) => eprintln!(
                    "build {}: cancelled {} job {} of build {} ({}), which this build's {} contains",
                    build.build_id,
                    platform.platform,
                    platform.job_id,
                    old.build_id,
                    old.source_commit,
                    build.source_commit
                ),
                Err(error) => eprintln!(
                    "build {}: {} job {} of superseded build {} could not be cancelled: {error}",
                    build.build_id, platform.platform, platform.job_id, old.build_id
                ),
            }
        }
    }
}
