//! Resume the immutable source already recorded by a release submission.

use crate::cli::release_submit::publish::signing::require_rollback_compatibility;
use crate::cli::release_submit::ReleaseResumeArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{self, ProductManifest};

use super::source::{build_path, build_uri, identity};
use super::state::load;
use super::submit::continue_run;

pub async fn resume(args: &ReleaseResumeArgs) -> Result<(), CmdError> {
    finish_run(&args.run_id, args.json).await
}

/// Walk a recorded run to its end: enqueue what was never submitted, wait for
/// the builds, sign, publish, deliver. `stado release resume` does this on
/// request; the control host's release agent does it on its own for every
/// run whose builds have finished, which is why `submit` no longer waits.
pub(crate) async fn finish_run(run_id: &str, json: bool) -> Result<(), CmdError> {
    let args = ReleaseResumeArgs {
        run_id: run_id.to_string(),
        json,
    };
    let args = &args;
    // source::identity takes 32 lowercase SHA-256 characters; validate that
    // existing identity contract before constructing any storage path.
    if args.run_id.len() != 32
        || !args
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CmdError::usage(
            "run ID must be 32 lowercase hexadecimal characters",
        ));
    }
    let run = load(&args.run_id)
        .await?
        .ok_or_else(|| CmdError::click(format!("release run {} does not exist", args.run_id)))?;
    if run.run_id != args.run_id
        || identity(
            &run.product,
            &run.version,
            run.channel,
            &run.source_sha256,
            &run.manifest_sha256,
        ) != args.run_id
    {
        return Err(CmdError::click("durable release run identity mismatch"));
    }
    // The manifest the run was made from is the build's staged copy; the run
    // names both the build and the coordinate, and they must agree.
    let Some(build_id) = run.build_id.as_deref() else {
        return Err(CmdError::click(format!(
            "release run {} predates build records and cannot be resumed; submit the same commit again with `stado release submit --source`",
            run.run_id
        )));
    };
    let path = build_path(&run.product, build_id, "manifest.json");
    if run.manifest_uri != build_uri(&run.product, build_id, "manifest.json") {
        return Err(CmdError::click("release run manifest coordinate mismatch"));
    }
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let bytes = store
        .read_bytes(&path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click(format!("release run manifest is missing: {path}")))?;
    if release_control::sha256_bytes(&bytes) != run.manifest_sha256 {
        return Err(CmdError::click("release run manifest digest mismatch"));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click("release run manifest disables releases"));
    };
    if manifest.product != run.product || !manifest.promotion.channels.contains(&run.channel) {
        return Err(CmdError::click(
            "recorded release manifest disagrees with the run",
        ));
    }
    require_rollback_compatibility(&manifest, &run.version).await?;
    // The same reconciler as submit verifies published bytes, retains pending
    // jobs, and retries only terminal failures. It never snapshots this cwd.
    continue_run(run, manifest, args.json, true).await
}
