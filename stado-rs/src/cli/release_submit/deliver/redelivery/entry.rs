//! `stado release redeliver` — resume or run one named delivery again.

use serde_json::Map;

use crate::cli::release_submit::builds::jobs::terminal::{job_output_tail, terminal};
use crate::cli::release_submit::builds::jobs::{input, secret_refs};
use crate::cli::release_submit::deliver::redelivery::finish::finish_redelivery;
use crate::cli::release_submit::deliver::redelivery::plan::plan_redelivery;
use crate::cli::release_submit::deliver::redelivery::transaction::{
    load_redelivery_transaction, replace_redelivery_transaction,
};
use crate::cli::release_submit::deliver::redelivery::RedeliveryStage;
use crate::cli::release_submit::deliver::{delivery_job_command, DeliveryRequest};
use crate::cli::release_submit::run::source::{run_path, run_uri};
use crate::cli::release_submit::run::state::{load, save};
use crate::cli::release_submit::ReleaseRedeliverArgs;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::release_control;
use crate::release_pipeline::ReleaseRunState;

/// Re-run one named delivery from an exact completed release run.
///
/// A fixed, separately serialized transaction fences the temporary run-state
/// transition without changing the backwards-compatible `ReleaseRun` schema.
/// Every boundary is durable and the caller's retry token resumes the same job.
pub async fn redeliver(args: &ReleaseRedeliverArgs) -> Result<(), CmdError> {
    if args.retry_token.is_empty() || args.retry_token.len() > 128 {
        return Err(CmdError::click(
            "--retry-token must contain between 1 and 128 bytes",
        ));
    }
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let token_sha = release_control::sha256_bytes(args.retry_token.as_bytes());
    let transaction_path = run_path(&args.product, &args.run_id, "redelivery.json");

    let mut run = load(&args.run_id)
        .await?
        .ok_or_else(|| CmdError::click(format!("release run {} does not exist", args.run_id)))?;
    if run.product != args.product {
        return Err(CmdError::click("release run belongs to another product"));
    }

    let mut loaded = load_redelivery_transaction(&store, &transaction_path).await?;
    if let Some((active, _)) = &loaded {
        if active.retry_token_sha256 != token_sha || active.delivery != args.delivery {
            if !active.stage.terminal() {
                return Err(CmdError::click(format!(
                    "release run has an active redelivery for {}",
                    active.delivery
                )));
            }
            if run.state != active.previous_run_state {
                return Err(CmdError::click(
                    "terminal redelivery transaction has not restored the release run",
                ));
            }
        }
    }
    let matching_transaction = loaded.as_ref().is_some_and(|(active, _)| {
        active.retry_token_sha256 == token_sha && active.delivery == args.delivery
    });
    if matching_transaction {
        let (active, version) = loaded
            .as_ref()
            .cloned()
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
        if finish_redelivery(
            args,
            &store,
            &transaction_path,
            run.clone(),
            active,
            version,
        )
        .await?
        .is_some()
        {
            return Ok(());
        }
    }

    let leaf = format!("deliveries/{}/redeliveries/{token_sha}", args.delivery);
    let request_path = run_path(&run.product, &run.run_id, &format!("{leaf}/request.json"));
    let request_uri = run_uri(&run.product, &run.run_id, &format!("{leaf}/request.json"));
    let (request, request_sha, consumer) = if matching_transaction {
        let active = &loaded
            .as_ref()
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?
            .0;
        let request_bytes = store
            .read_bytes(&request_path)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
            .ok_or_else(|| CmdError::click("redelivery request disappeared"))?;
        if release_control::sha256_bytes(&request_bytes) != active.request_sha256 {
            return Err(CmdError::click("redelivery request digest mismatch"));
        }
        let request: DeliveryRequest = serde_json::from_slice(&request_bytes)?;
        if request.run_id != run.run_id
            || request.product != run.product
            || request.name != args.delivery
        {
            return Err(CmdError::click("redelivery request identity mismatch"));
        }
        (
            request,
            active.request_sha256.clone(),
            active.pinned_consumer.clone(),
        )
    } else {
        let planned = plan_redelivery(
            args,
            &store,
            &run,
            &transaction_path,
            &request_path,
            &token_sha,
            loaded,
        )
        .await?;
        loaded = planned.loaded;
        (planned.request, planned.request_sha, planned.consumer)
    };
    let mut resolved = Map::new();
    resolved.insert(
        "request".into(),
        input(&request_uri, "delivery-request.json", &request_sha),
    );
    resolved.insert(
        "archive".into(),
        input(
            &request.archive_uri,
            "release.tar.gz",
            &request.archive_sha256,
        ),
    );
    resolved.insert(
        "source".into(),
        input(
            &run_uri(&run.product, &run.run_id, "inputs/source.tar.gz"),
            "source.tar.gz",
            &request.source_sha256,
        ),
    );
    let (mut transaction, mut transaction_version) =
        loaded.ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    let options = SubmitOptions {
        pinned_host: consumer,
        priority: crate::primitives::constants::RELEASE_JOB_PRIORITY,
        run_id: stable_run_id(
            "release-redelivery",
            &format!("{}\0{}\0{}", run.run_id, request.name, token_sha),
        ),
        output_uri: run_uri(&run.product, &run.run_id, &format!("{leaf}/output")),
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&request.secret_env),
        ..Default::default()
    };

    if transaction.stage == RedeliveryStage::IntentCreated {
        if run.state == transaction.previous_run_state {
            run.state = ReleaseRunState::Delivering;
            save(&mut run).await?;
        } else if run.state != ReleaseRunState::Delivering {
            return Err(CmdError::click(
                "release run changed before the redelivery fence was installed",
            ));
        }
        transaction.stage = RedeliveryStage::RunReopened;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    if transaction.stage == RedeliveryStage::RunReopened {
        let command = delivery_job_command(&run.product).to_string();
        let job = submit_batch(std::slice::from_ref(&command), &options)
            .await?
            .pop()
            .ok_or_else(|| CmdError::click("durable redelivery submission returned no job"))?;
        transaction.job_id = Some(job.job_id);
        transaction.stage = RedeliveryStage::Submitted;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    if transaction.stage == RedeliveryStage::Submitted {
        let job_id = transaction
            .job_id
            .as_deref()
            .ok_or_else(|| CmdError::click("submitted redelivery has no job id"))?;
        let job = terminal(&store, job_id).await?;
        let ok = matches!(
            job.state.as_str(),
            job_state::COMPLETED | job_state::UPLOADED
        );
        if ok {
            let receipt = store
                .read_bytes(&format!("status/{job_id}/output/delivery-receipt.json"))
                .await?
                .ok_or_else(|| CmdError::click("redelivery produced no delivery receipt"))?;
            transaction.receipt_sha256 = Some(release_control::sha256_bytes(&receipt));
        } else {
            transaction.failure = Some(format!(
                "{}{}",
                job.error.unwrap_or_else(|| job.state.clone()),
                job_output_tail(&store, job_id).await
            ));
        }
        transaction.stage = RedeliveryStage::Terminal;
        replace_redelivery_transaction(
            &store,
            &transaction_path,
            &transaction_version,
            &transaction,
        )
        .await?;
        (transaction, transaction_version) = load_redelivery_transaction(&store, &transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
    }

    finish_redelivery(
        args,
        &store,
        &transaction_path,
        run,
        transaction,
        transaction_version,
    )
    .await?
    .ok_or_else(|| CmdError::click("redelivery stopped before a terminal transaction"))?;
    Ok(())
}
