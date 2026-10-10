//! Restore the release run the redelivery fence re-opened, and report the
//! terminal transaction.

use serde_json::json;

use crate::cli::release_submit::deliver::redelivery::transaction::{
    load_redelivery_transaction, replace_redelivery_transaction,
};
use crate::cli::release_submit::deliver::redelivery::{RedeliveryStage, RedeliveryTransaction};
use crate::cli::release_submit::deliver::{verified_receipt, DeliveryRequest};
use crate::cli::release_submit::run::source::run_path;
use crate::cli::release_submit::run::state::save;
use crate::cli::release_submit::ReleaseRedeliverArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{DeliveryRunState, ReleaseRun, ReleaseRunState};

pub(super) async fn finish_redelivery(
    args: &ReleaseRedeliverArgs,
    store: &JobStorage,
    transaction_path: &str,
    mut run: ReleaseRun,
    mut transaction: RedeliveryTransaction,
    mut transaction_version: String,
) -> Result<Option<()>, CmdError> {
    if transaction.failure.is_none()
        && matches!(
            transaction.stage,
            RedeliveryStage::Terminal | RedeliveryStage::RunRestored | RedeliveryStage::Completed
        )
    {
        verify_retained_proof(store, &run, &transaction)
            .await
            .map_err(|cause| {
                CmdError::click(cause).stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
    }
    if transaction.stage == RedeliveryStage::Terminal {
        if transaction.failure.is_none() {
            let updated = run
                .deliveries
                .get_mut(&transaction.delivery)
                .ok_or_else(|| {
                    CmdError::click("release delivery disappeared")
                        .stating(crate::primitives::failure::FailureCode::InfraDown)
                })?;
            updated.job_id = transaction.job_id.clone().ok_or_else(|| {
                CmdError::click("terminal redelivery has no job id")
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            updated.output_prefix = format!("status/{}/output/", updated.job_id);
            updated.state = DeliveryRunState::Passed;
            updated.receipt_sha256 = transaction.receipt_sha256.clone();
            updated.failure = None;
        }
        if run.state == ReleaseRunState::Delivering {
            run.state = transaction.previous_run_state.clone();
            save(&mut run).await?;
        } else if run.state != transaction.previous_run_state {
            return Err(CmdError::refused(
                "release run changed before redelivery restoration",
            ));
        }
        transaction.stage = RedeliveryStage::RunRestored;
        replace_redelivery_transaction(store, transaction_path, &transaction_version, &transaction)
            .await?;
        (transaction, transaction_version) = load_redelivery_transaction(store, transaction_path)
            .await?
            .ok_or_else(|| {
                CmdError::click("redelivery transaction disappeared")
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
    }
    if transaction.stage == RedeliveryStage::RunRestored {
        transaction.stage = if transaction.failure.is_some() {
            RedeliveryStage::Failed
        } else {
            RedeliveryStage::Completed
        };
        replace_redelivery_transaction(store, transaction_path, &transaction_version, &transaction)
            .await?;
    }
    if !transaction.stage.terminal() {
        return Ok(None);
    }
    if let Some(failure) = transaction.failure {
        return Err(CmdError::click(format!(
            "redelivery job {} failed: {failure}",
            transaction.job_id.as_deref().unwrap_or("unknown")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let job_id = transaction.job_id.ok_or_else(|| {
        CmdError::click("completed redelivery has no job id")
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "passed",
                "product": run.product,
                "version": run.version,
                "run_id": run.run_id,
                "delivery": transaction.delivery,
                "job_id": job_id,
                "receipt_sha256": transaction.receipt_sha256,
            }))?
        );
    } else {
        println!(
            "redelivered {} {} run {} delivery {} with job {}",
            run.product, run.version, run.run_id, transaction.delivery, job_id
        );
    }
    Ok(Some(()))
}

/// Replaying a completed transaction must still prove its exact retained bytes.
/// A read failure refuses the replay without replacing its accepted history.
async fn verify_retained_proof(
    store: &JobStorage,
    run: &ReleaseRun,
    transaction: &RedeliveryTransaction,
) -> Result<(), String> {
    let job_id = transaction
        .job_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "redelivery {} has no retained job identity",
                transaction.delivery
            )
        })?;
    let accepted = transaction
        .receipt_sha256
        .as_deref()
        .ok_or_else(|| format!("redelivery job {job_id} has no accepted receipt digest"))?;
    let request_path = run_path(
        &run.product,
        &run.run_id,
        &format!(
            "deliveries/{}/redeliveries/{}/request.json",
            transaction.delivery, transaction.retry_token_sha256
        ),
    );
    let bytes = store
        .read_bytes(&request_path)
        .await
        .map_err(|cause| format!("cannot read redelivery request {request_path}: {cause}"))?
        .ok_or_else(|| format!("redelivery request {request_path} is absent"))?;
    let observed = crate::release_control::sha256_bytes(&bytes);
    if observed != transaction.request_sha256 {
        return Err(format!(
            "redelivery request {request_path} hashes to {observed}, but its transaction accepted {}",
            transaction.request_sha256
        ));
    }
    let request: DeliveryRequest = serde_json::from_slice(&bytes)
        .map_err(|cause| format!("cannot decode redelivery request {request_path}: {cause}"))?;
    for (field, written, expected) in [
        ("run", request.run_id.as_str(), run.run_id.as_str()),
        ("product", request.product.as_str(), run.product.as_str()),
        ("version", request.version.as_str(), run.version.as_str()),
        (
            "delivery",
            request.name.as_str(),
            transaction.delivery.as_str(),
        ),
        (
            "source uri",
            request.source_uri.as_str(),
            run.source_uri.as_str(),
        ),
        (
            "source sha256",
            request.source_sha256.as_str(),
            run.source_sha256.as_str(),
        ),
    ] {
        if written != expected {
            return Err(format!("redelivery request {request_path} names {field} {written:?}, expected {expected:?}"));
        }
    }
    let receipt_path = format!("status/{job_id}/output/delivery-receipt.json");
    let receipt = store
        .read_bytes(&receipt_path)
        .await
        .map_err(|cause| format!("cannot read redelivery receipt {receipt_path}: {cause}"))?;
    let observed = verified_receipt(receipt.as_deref(), &request, job_id)?;
    if observed != accepted {
        return Err(format!(
            "redelivery receipt {receipt_path} hashes to {observed}, but its transaction accepted {accepted}"
        ));
    }
    Ok(())
}
