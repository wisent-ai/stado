//! `stado build status` and `stado build list`: the read side of the build
//! records, joined to what the queue says about each platform's job.

use std::collections::BTreeMap;

use serde_json::Value;

use super::progress::Progress;

use crate::cli::build_cmd::{require_build_id, BuildListArgs, BuildStatusArgs};
use crate::cli::release_submit::{build_path, load_build, refresh_build, save_build, terminal_job};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{self, BuildRun, BuildRunState, PlatformRunState, ProductManifest};

/// Where every build object lives, and its leaf.
const BUILD_STATE_PREFIX: &str = "runs/build/";
const BUILD_STATE_LEAF: &str = "/run.json";
/// How many build objects a product-filtered listing reads before it stops:
/// the product is in the body, not the path, and the whole history is not a
/// bounded question.
const SCAN_WINDOW: usize = 120;

/// One line's worth of what the platforms did.
pub(super) fn summary(build: &BuildRun) -> String {
    let mut counts = [0usize; 4];
    for platform in build.platforms.values() {
        let slot = match platform.state {
            PlatformRunState::Submitted => 0,
            PlatformRunState::Qualified => 1,
            PlatformRunState::Failed => 2,
            PlatformRunState::Published => 3,
        };
        counts[slot] += 1;
    }
    format!(
        "{} platform build(s) queued or running, {} passed, {} failed",
        counts[0] + counts[3],
        counts[1],
        counts[2]
    )
}

/// The build as it stands now: its record brought up to date from the queue
/// and its jobs' receipts, and saved when that changed anything. With
/// `wait`, every platform still building is followed to its job's end first.
pub(crate) async fn current_build(build_id: &str, wait: bool) -> Result<BuildRun, CmdError> {
    require_build_id(build_id)?;
    let mut build = load_build(build_id)
        .await?
        .ok_or_else(|| CmdError::click(format!("build {build_id} does not exist")))?;
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let manifest_path = build_path(&build.product, &build.build_id, "manifest.json");
    let bytes = store
        .read_bytes(&manifest_path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click(format!("build manifest is missing: {manifest_path}")))?;
    if release_control::sha256_bytes(&bytes) != build.manifest_sha256 {
        return Err(CmdError::click("build manifest digest mismatch"));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click("build manifest disables releases"));
    };
    if wait {
        build = queued_build(build, manifest.platforms.len()).await?;
        for platform in build.platforms.values() {
            if platform.state == PlatformRunState::Submitted {
                terminal_job(&store, &platform.job_id).await?;
            }
        }
    }
    let before = build.clone();
    refresh_build(&store, &mut build, &manifest).await?;
    if build != before {
        save_build(&mut build).await?;
    }
    Ok(build)
}

/// How long `--wait` lets a submission take to queue every platform's job.
/// Staging and queueing took six and a half minutes on 2026-09-23; a
/// submitter that died between recording the build and queueing its jobs
/// leaves a record that would otherwise be waited on forever.
const QUEUEING_LIMIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);
const QUEUEING_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// The build once its submission has queued a job for every platform the
/// manifest declares, or recorded why it could not. A build is recorded
/// before its jobs are queued, so `--wait` read in that gap used to answer
/// `waiting` at once, which is the one answer it promises never to give.
async fn queued_build(mut build: BuildRun, declared: usize) -> Result<BuildRun, CmdError> {
    let started = std::time::Instant::now();
    while build.platforms.len() < declared
        && build.state == BuildRunState::Waiting
        && build.failure.is_none()
    {
        if started.elapsed() >= QUEUEING_LIMIT {
            return Err(CmdError::click(format!(
                "build {} queued {} of {declared} platform job(s) in {} minutes; its submitter \
                 stopped before queueing the rest. `stado build submit` with the same commit and \
                 version queues them",
                build.build_id,
                build.platforms.len(),
                QUEUEING_LIMIT.as_secs() / 60,
            )));
        }
        tokio::time::sleep(QUEUEING_POLL).await;
        build = load_build(&build.build_id)
            .await?
            .ok_or_else(|| CmdError::click(format!("build {} disappeared", build.build_id)))?;
    }
    Ok(build)
}

fn print_build(build: &BuildRun, progress: &BTreeMap<String, Progress>) {
    println!(
        "build {} product={} version={} commit={} state={}: {}",
        build.build_id,
        build.product,
        build.version,
        build.source_commit,
        build.state.word(),
        summary(build)
    );
    for (name, platform) in &build.platforms {
        let state = match platform.state {
            PlatformRunState::Submitted => "building",
            PlatformRunState::Qualified => "passed",
            PlatformRunState::Failed => "failed",
            PlatformRunState::Published => "published",
        };
        println!(
            "  {name}: builder={} job={} state={state}{}{}",
            platform.builder,
            platform.job_id,
            platform
                .artifact_sha256
                .as_deref()
                .map(|sha| format!(" artifact_sha256={sha}"))
                .unwrap_or_default(),
            platform
                .failure
                .as_deref()
                .map(|failure| format!(" failure: {failure}"))
                .unwrap_or_default()
        );
        if let Some(progress) = progress.get(name) {
            for line in progress.lines(platform.state == PlatformRunState::Submitted) {
                println!("    {line}");
            }
        }
    }
    if let Some(failure) = &build.failure {
        println!("  failure: {failure}");
    }
}

/// What every platform's job did and, while it builds, what it is doing.
async fn platform_progress(build: &BuildRun) -> Result<BTreeMap<String, Progress>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let now = chrono::Utc::now();
    let mut progress = BTreeMap::new();
    for (name, platform) in &build.platforms {
        progress.insert(
            name.clone(),
            super::progress::read(&store, &platform.job_id, now).await,
        );
    }
    Ok(progress)
}

/// How often a text `--wait` rereads the jobs to report a new step. A step
/// lasts from seconds to half an hour; a quarter of a minute says when one
/// started without rereading every log each heartbeat.
const FOLLOW_POLL: std::time::Duration = std::time::Duration::from_secs(15);

/// Follow a build on stderr until no platform is still building: each
/// platform's queue wait, every step as it starts and as it ends, and how
/// long it took. `--json` stays one document and does not follow.
async fn follow(build_id: &str) -> Result<(), CmdError> {
    let mut said = std::collections::HashSet::new();
    loop {
        let build = current_build(build_id, false).await?;
        let progress = platform_progress(&build).await?;
        for (name, progress) in &progress {
            let mut lines = progress.lines(false);
            if let Some(running) = &progress.running {
                lines.push(format!(
                    "step {}: started at {}",
                    running.name, running.since
                ));
                if let Some(blocked) = &running.blocked_on {
                    lines.push(format!("step {}: blocked: {blocked}", running.name));
                }
            }
            for line in lines {
                if said.insert(format!("{name}\0{line}")) {
                    eprintln!("[build status] {name}: {line}");
                }
            }
        }
        // A build whose submitter has not queued a job yet has nothing to
        // follow; `current_build` with `wait` owns that wait and its limit.
        // A build that already failed on one platform still has the other's
        // job to wait for, and `--wait` waits for it: following stopped at
        // the failure used to leave that wait silent for as long as the
        // other job ran, which was an hour twice on one day.
        let building = build
            .platforms
            .values()
            .any(|platform| platform.state == PlatformRunState::Submitted);
        if !building {
            return Ok(());
        }
        tokio::time::sleep(FOLLOW_POLL).await;
    }
}

pub(super) async fn status(args: &BuildStatusArgs) -> Result<(), CmdError> {
    if args.wait && !args.json {
        follow(&args.build_id).await?;
    }
    let build = current_build(&args.build_id, args.wait).await?;
    let progress = platform_progress(&build).await?;
    if args.json {
        let mut document = serde_json::to_value(&build)?;
        document["progress"] = serde_json::to_value(&progress)?;
        println!("{}", serde_json::to_string_pretty(&document)?)
    } else {
        print_build(&build, &progress)
    }
    Ok(())
}

/// The newest `limit` builds the product filter admits, newest first, as
/// recorded: a listing does not read every build's jobs.
pub(crate) async fn recent_builds(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<BuildRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut blobs = store
        .list_blobs_with_meta(BUILD_STATE_PREFIX)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .into_iter()
        .filter(|blob| blob.name.ends_with(BUILD_STATE_LEAF))
        .collect::<Vec<_>>();
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    let mut builds = Vec::new();
    for blob in blobs.iter().take(SCAN_WINDOW) {
        if builds.len() >= limit {
            break;
        }
        let Some(text) = store
            .download_text(&blob.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        let Ok(build) = serde_json::from_str::<BuildRun>(&text) else {
            continue;
        };
        if product.is_none_or(|selected| build.product == selected) {
            builds.push(build);
        }
    }
    Ok(builds)
}

pub(super) async fn list(args: &BuildListArgs) -> Result<(), CmdError> {
    let builds = recent_builds(args.product.as_deref(), args.limit).await?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Array(
                builds
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()?
            ))?
        )
    } else if builds.is_empty() {
        println!("no builds recorded");
    } else {
        for build in &builds {
            println!(
                "{} {} {} {} {} {}",
                build.updated_at,
                build.build_id,
                build.product,
                build.version,
                build.state.word(),
                summary(build)
            );
        }
    }
    Ok(())
}
