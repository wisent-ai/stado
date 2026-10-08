//! `stado build list`: the newest build records, each unfinished one read
//! against its jobs first.

use serde_json::Value;

use super::summary;
use crate::cli::build_cmd::BuildListArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{BuildRun, BuildRunState, PlatformRunState};

/// Where every build object lives, and its leaf.
const BUILD_STATE_PREFIX: &str = "runs/build/";
const BUILD_STATE_LEAF: &str = "/run.json";

/// The builds the product filter admits, newest first: every one, or the
/// newest `limit` when the caller names a count. A finished build is listed as
/// recorded; one still waiting, or with a platform still submitted, is read
/// against its jobs and saved first, exactly as `stado build status` does.
/// Nothing else advances a build record that no release run consumes, so a
/// listing of the records alone reported builds `waiting` hours after every
/// one of their jobs had ended.
pub(in crate::cli::build_cmd) async fn recent_builds(
    product: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<BuildRun>, CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let mut blobs = store
        .list_blobs_with_meta(BUILD_STATE_PREFIX)
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .filter(|blob| blob.name.ends_with(BUILD_STATE_LEAF))
        .collect::<Vec<_>>();
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    let mut builds = Vec::new();
    for blob in &blobs {
        if limit.is_some_and(|limit| builds.len() >= limit) {
            break;
        }
        let Some(text) = store
            .download_text(&blob.name)
            .await
            .map_err(CmdError::from)?
        else {
            continue;
        };
        let Ok(build) = serde_json::from_str::<BuildRun>(&text) else {
            continue;
        };
        if product.is_none_or(|selected| build.product == selected) {
            let unfinished = build.state == BuildRunState::Waiting
                || build
                    .platforms
                    .values()
                    .any(|platform| platform.state == PlatformRunState::Submitted);
            if unfinished {
                let current =
                    super::current_build(&build.build_id, false)
                        .await
                        .map_err(|error| {
                            error.within(format!(
                                "cannot read build {} against its jobs",
                                build.build_id
                            ))
                        })?;
                builds.push(current);
            } else {
                builds.push(build);
            }
        }
    }
    Ok(builds)
}

pub(in crate::cli::build_cmd) async fn list(args: &BuildListArgs) -> Result<(), CmdError> {
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
