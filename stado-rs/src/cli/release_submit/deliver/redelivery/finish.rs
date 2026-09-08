//! Restore the release run the redelivery fence re-opened, and report the
//! terminal transaction.

use serde_json::json;

use crate::cli::release_submit::deliver::redelivery::transaction::{
    load_redelivery_transaction, replace_redelivery_transaction,
};
use crate::cli::release_submit::deliver::redelivery::{RedeliveryStage, RedeliveryTransaction};
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
    if transaction.stage == RedeliveryStage::Terminal {
        if transaction.failure.is_none() {
            let updated = run
                .deliveries
                .get_mut(&transaction.delivery)
                .ok_or_else(|| CmdError::click("release delivery disappeared"))?;
            updated.job_id = transaction
                .job_id
                .clone()
                .ok_or_else(|| CmdError::click("terminal redelivery has no job id"))?;
            updated.output_prefix = format!("status/{}/output/", updated.job_id);
            updated.state = DeliveryRunState::Passed;
            updated.receipt_sha256 = transaction.receipt_sha256.clone();
            updated.failure = None;
        }
        if run.state == ReleaseRunState::Delivering {
            run.state = transaction.previous_run_state.clone();
            save(&mut run).await?;
        } else if run.state != transaction.previous_run_state {
            return Err(CmdError::click(
                "release run changed before redelivery restoration",
            ));
        }
        transaction.stage = RedeliveryStage::RunRestored;
        replace_redelivery_transaction(store, transaction_path, &transaction_version, &transaction)
            .await?;
        (transaction, transaction_version) = load_redelivery_transaction(store, transaction_path)
            .await?
            .ok_or_else(|| CmdError::click("redelivery transaction disappeared"))?;
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
        )));
    }
    let job_id = transaction
        .job_id
        .ok_or_else(|| CmdError::click("completed redelivery has no job id"))?;
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
