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
/// `wait`, the read holds on the store's change watch until its submission
/// has queued every platform's job and every job has ended.
pub(crate) async fn current_build(build_id: &str, wait: bool) -> Result<BuildRun, CmdError> {
    require_build_id(build_id)?;
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    // Armed before the build is first read, so nothing written between that
    // read and the wait is missed.
    let record = format!("runs/build/{build_id}");
    let mut watched: Vec<&str> = crate::queue::runs::TERMINAL_PREFIXES.to_vec();
    watched.push(&record);
    let mut watch = if wait {
        Some(store.watch_prefixes(&watched).map_err(|error| {
            CmdError::click(format!("build status --wait cannot hold: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?)
    } else {
        None
    };
    let mut build = load_build(build_id).await?.ok_or_else(|| {
        CmdError::click(format!("build {build_id} does not exist"))
            .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    let manifest_path = build_path(&build.product, &build.build_id, "manifest.json");
    let bytes = store
        .read_bytes(&manifest_path)
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!("build manifest is missing: {manifest_path}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    if release_control::sha256_bytes(&bytes) != build.manifest_sha256 {
        return Err(CmdError::click("build manifest digest mismatch")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::refused("build manifest disables releases"));
    };
    if let Some(mut armed) = watch.take() {
        // A build is recorded before its jobs are queued, so a wait read in
        // that gap used to answer `waiting` at once, which is the one answer
        // it promises never to give.
        while build.platforms.len() < manifest.platforms.len()
            && build.state == BuildRunState::Waiting
            && build.failure.is_none()
        {
            armed = changed(armed, build_id).await?;
            build = load_build(build_id).await?.ok_or_else(|| {
                CmdError::click(format!("build {build_id} disappeared"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        }
        for platform in build.platforms.values() {
            if platform.state == PlatformRunState::Submitted {
                while read_terminal_job(&store, &platform.job_id).await?.is_none() {
                    armed = changed(armed, build_id).await?;
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

/// Block until the store reports a change under the watched prefixes, and
/// hand the watch back for the next wait.
async fn changed(
    armed: Box<dyn crate::queue::ChangeWatch>,
    build_id: &str,
) -> Result<Box<dyn crate::queue::ChangeWatch>, CmdError> {
    tokio::task::spawn_blocking(move || {
        let mut armed = armed;
        armed.next().map(|()| armed)
    })
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "the change watch on build {build_id} stopped: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?
    .map_err(|error| {
        CmdError::click(format!(
            "the change watch on build {build_id} failed: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
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
    let store = JobStorage::new().await.map_err(CmdError::from)?;
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

/// Follow a build on stderr until no platform is still building, woken by the
/// store's change watch on the build record, the terminal prefixes and every
/// platform job's output: each platform's queue wait, every step as it starts
/// and as it ends, and how long it took. `--json` stays one document and does
/// not follow.
async fn follow(build_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let mut said = std::collections::HashSet::new();
    let mut armed: Option<Box<dyn crate::queue::ChangeWatch>> = None;
    loop {
        let build = current_build(build_id, false).await?;
        if armed.is_none() {
            let mut watched: Vec<String> = crate::queue::runs::TERMINAL_PREFIXES
                .iter()
                .map(|prefix| prefix.to_string())
                .collect();
            watched.push(format!("runs/build/{build_id}"));
            watched.extend(
                build
                    .platforms
                    .values()
                    .map(|platform| format!("status/{}/output", platform.job_id)),
            );
            let watched: Vec<&str> = watched.iter().map(String::as_str).collect();
            armed = Some(store.watch_prefixes(&watched).map_err(|error| {
                CmdError::click(format!("build status --wait cannot follow: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?);
            // The first read came before the watch: read again under it, so
            // a change in that gap is not waited for in vain.
            continue;
        }
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
        if let Some(watch) = armed.take() {
            armed = Some(changed(watch, build_id).await?);
        }
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
