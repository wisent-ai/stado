//! Resume the immutable source already recorded by a release submission.

use crate::cli::release_submit::publish::signing::require_rollback_compatibility;
use crate::cli::release_submit::ReleaseResumeArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{self, ProductManifest};

use super::source::{identity, run_path, run_uri};
use super::state::load;
use super::submit::continue_run;

pub async fn resume(args: &ReleaseResumeArgs) -> Result<(), CmdError> {
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
    let path = run_path(&run.product, &run.run_id, "manifest.json");
    if run.manifest_uri != run_uri(&run.product, &run.run_id, "manifest.json") {
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
    continue_run(run, manifest, args.json).await
}
