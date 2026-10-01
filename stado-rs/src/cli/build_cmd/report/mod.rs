//! `stado build status` and `stado build list`: the read side of the build
//! records, joined to what the queue says about each platform's job.

mod list;
mod progress;
pub(super) mod refusals;

use std::collections::BTreeMap;

use progress::Progress;

use crate::cli::build_cmd::{require_build_id, BuildStatusArgs};
use crate::cli::release_submit::{
    build_path, load_build, read_terminal_job, refresh_build, save_build,
};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{self, BuildRun, BuildRunState, PlatformRunState, ProductManifest};

pub(super) use list::{list, recent_builds};

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
/// `wait`, the build is re-read at that period until its submission has
/// queued every platform's job and every job has ended.
pub(crate) async fn current_build(
    build_id: &str,
    wait: Option<std::time::Duration>,
) -> Result<BuildRun, CmdError> {
    require_build_id(build_id)?;
    let mut build = load_build(build_id)
        .await?
        .ok_or_else(|| CmdError::refused(format!("build {build_id} does not exist")))?;
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
    if let Some(period) = wait {
        build = queued_build(build, manifest.platforms.len(), period).await?;
        for platform in build.platforms.values() {
            if platform.state == PlatformRunState::Submitted {
                while read_terminal_job(&store, &platform.job_id).await?.is_none() {
                    tokio::time::sleep(period).await;
                }
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

/// The build once its submission has queued a job for every platform the
/// manifest declares, or recorded why it could not, re-read every `period`.
/// A build is recorded before its jobs are queued, so a wait read in that gap
/// used to answer `waiting` at once, which is the one answer it promises
/// never to give.
async fn queued_build(
    mut build: BuildRun,
    declared: usize,
    period: std::time::Duration,
) -> Result<BuildRun, CmdError> {
    while build.platforms.len() < declared
        && build.state == BuildRunState::Waiting
        && build.failure.is_none()
    {
        tokio::time::sleep(period).await;
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
            progress::read(&store, &platform.job_id, now).await,
        );
    }
    Ok(progress)
}

/// Follow a build on stderr until no platform is still building, re-reading
/// it every `period`: each platform's queue wait, every step as it starts and
/// as it ends, and how long it took. `--json` stays one document and does not
/// follow.
async fn follow(build_id: &str, period: std::time::Duration) -> Result<(), CmdError> {
    let mut said = std::collections::HashSet::new();
    loop {
        let build = current_build(build_id, None).await?;
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
        // follow; `current_build` with a wait owns that wait. A build that
        // already failed on one platform still has the other's job to wait
        // for, and the wait waits for it: following stopped at the failure
        // used to leave that wait silent for as long as the other job ran,
        // which was an hour twice on one day.
        let building = build
            .platforms
            .values()
            .any(|platform| platform.state == PlatformRunState::Submitted);
        if !building {
            return Ok(());
        }
        tokio::time::sleep(period).await;
    }
}

pub(super) async fn status(args: &BuildStatusArgs) -> Result<(), CmdError> {
    let wait = args.wait_seconds.map(std::time::Duration::from_secs);
    if let (Some(period), false) = (wait, args.json) {
        follow(&args.build_id, period).await?;
    }
    let build = current_build(&args.build_id, wait).await?;
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
