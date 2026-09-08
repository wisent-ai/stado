//! Plan a redelivery that has no transaction yet: prove the run is the
//! newest completed candidate, write the immutable request, and record the
//! intent.

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::builder::{builder, target_consumer};
use crate::cli::release_submit::deliver::redelivery::transaction::{
    create_redelivery_transaction, load_redelivery_transaction, replace_redelivery_transaction,
};
use crate::cli::release_submit::deliver::redelivery::{RedeliveryStage, RedeliveryTransaction};
use crate::cli::release_submit::deliver::DeliveryRequest;
use crate::cli::release_submit::run::source::{queue_immutable, run_path};
use crate::cli::release_submit::run::state::latest_submitted_run;
use crate::cli::release_submit::ReleaseRedeliverArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{
    self, DeliveryRunState, PipelineChannel, PlatformRunState, ProductManifest, ReleaseRun,
    ReleaseRunState,
};

/// The immutable request this redelivery will submit, its digest, the
/// consumer it is pinned to, and the transaction that now records the intent.
pub(super) struct PlannedRedelivery {
    pub(super) request: DeliveryRequest,
    pub(super) request_sha: String,
    pub(super) consumer: String,
    pub(super) loaded: Option<(RedeliveryTransaction, String)>,
}

pub(super) async fn plan_redelivery(
    args: &ReleaseRedeliverArgs,
    store: &JobStorage,
    run: &ReleaseRun,
    transaction_path: &str,
    request_path: &str,
    token_sha: &str,
    loaded: Option<(RedeliveryTransaction, String)>,
) -> Result<PlannedRedelivery, CmdError> {
    let latest = latest_submitted_run(&args.product)
        .await?
        .ok_or_else(|| CmdError::click("product has no submitted release run"))?;
    if latest.run_id != run.run_id
        || latest.source_sha256 != run.source_sha256
        || latest.version != run.version
    {
        return Err(CmdError::click(
            "only the newest exact submitted run may be redelivered",
        ));
    }
    if run.channel != PipelineChannel::Candidate {
        return Err(CmdError::click(
            "redelivery is restricted to completed candidate runs",
        ));
    }
    if run.state != ReleaseRunState::Completed {
        return Err(CmdError::click(
            "a new redelivery requires the latest release run to be completed",
        ));
    }
    let manifest_bytes = store
        .read_bytes(&run_path(&run.product, &run.run_id, "manifest.json"))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("release run manifest is missing"))?;
    if release_control::sha256_bytes(&manifest_bytes) != run.manifest_sha256 {
        return Err(CmdError::click("release run manifest digest mismatch"));
    }
    let manifest =
        match release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)? {
            ProductManifest::Release(manifest) => manifest,
            ProductManifest::NonRelease(_) => {
                return Err(CmdError::click("release run manifest disables releases"))
            }
        };
    let delivery = manifest
        .deliveries
        .iter()
        .find(|delivery| delivery.name == args.delivery)
        .ok_or_else(|| CmdError::click(format!("delivery {:?} is not declared", args.delivery)))?;
    let original = run
        .deliveries
        .get(&delivery.name)
        .ok_or_else(|| CmdError::click("release run never completed that delivery"))?;
    if original.state != DeliveryRunState::Passed {
        return Err(CmdError::click(
            "redelivery requires an originally passed delivery",
        ));
    }
    let artifact =
        release_cmd::verified_artifact_for_submit(&run.product, &run.version, &delivery.platform)
            .await?;
    let platform = run
        .platforms
        .get(&delivery.platform)
        .ok_or_else(|| CmdError::click("delivery platform is absent from the release run"))?;
    if platform.state != PlatformRunState::Published
        || platform.artifact_sha256.as_deref() != Some(artifact.artifact_sha256.as_str())
        || platform.release_manifest_sha256.as_deref() != Some(artifact.manifest_sha256.as_str())
    {
        return Err(CmdError::click(
            "published artifact no longer matches the release run",
        ));
    }
    let request = DeliveryRequest {
        schema_version: 1,
        run_id: run.run_id.clone(),
        name: delivery.name.clone(),
        product: run.product.clone(),
        version: run.version.clone(),
        platform: delivery.platform.clone(),
        argv: delivery.argv.clone(),
        required: delivery.required,
        secret_env: delivery.secret_env.clone(),
        source_path: "source.tar.gz".into(),
        source_uri: run.source_uri.clone(),
        source_sha256: run.source_sha256.clone(),
        archive_path: "release.tar.gz".into(),
        archive_uri: artifact.archive_uri.clone(),
        archive_sha256: artifact.artifact_sha256.clone(),
        manifest_uri: artifact.manifest_uri.clone(),
        manifest_sha256: artifact.manifest_sha256.clone(),
    };
    let request_bytes = serde_json::to_vec(&request)?;
    let request_sha = release_control::sha256_bytes(&request_bytes);
    queue_immutable(request_path, &request_bytes).await?;
    let consumer = if delivery.target.is_empty() {
        builder(
            &manifest.platforms[&delivery.platform].runner_platform,
            None,
        )
        .await?
        .1
    } else {
        target_consumer(&delivery.target).await?
    };
    let transaction = RedeliveryTransaction {
        schema_version: 1,
        retry_token_sha256: token_sha.to_owned(),
        delivery: args.delivery.clone(),
        previous_run_state: run.state.clone(),
        pinned_consumer: consumer.clone(),
        request_sha256: request_sha.clone(),
        stage: RedeliveryStage::IntentCreated,
        job_id: None,
        receipt_sha256: None,
        failure: None,
    };
    match loaded {
        None => create_redelivery_transaction(store, transaction_path, &transaction).await?,
        Some((_, version)) => {
            replace_redelivery_transaction(store, transaction_path, &version, &transaction).await?
        }
    }
    let loaded = load_redelivery_transaction(store, transaction_path).await?;
    Ok(PlannedRedelivery {
        request,
        request_sha,
        consumer,
        loaded,
    })
}
