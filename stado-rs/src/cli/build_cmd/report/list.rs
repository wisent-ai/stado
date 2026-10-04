//! `stado build list`: the newest build records, as recorded.

use serde_json::Value;

use super::summary;
use crate::cli::build_cmd::BuildListArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::BuildRun;

/// Where every build object lives, and its leaf.
const BUILD_STATE_PREFIX: &str = "runs/build/";
const BUILD_STATE_LEAF: &str = "/run.json";
/// How many build objects a product-filtered listing reads before it stops:
/// the product is in the body, not the path, and the whole history is not a
/// bounded question.
const SCAN_WINDOW: usize = 120;

/// The newest `limit` builds the product filter admits, newest first, as
/// recorded: a listing does not read every build's jobs.
pub(in crate::cli::build_cmd) async fn recent_builds(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<BuildRun>, CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
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
