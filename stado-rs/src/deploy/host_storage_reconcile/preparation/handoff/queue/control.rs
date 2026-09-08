use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn restore_queue_control(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
    rollback: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    if fence.queue.was_paused {
        fence.queue.resumed = true;
        return Ok(());
    }
    if fence.queue.restoration.is_none() {
        let owned = fence
            .queue
            .pause
            .as_ref()
            .filter(|effect| effect.status == "applied")
            .ok_or_else(|| {
                DeployError("queue restoration has no exact owned pause receipt".to_string())
            })?;
        let current = store
            .read_text_versioned(crate::queue::control::CONTROL_BLOB)
            .await
            .map_err(|error| {
                DeployError(format!(
                    "cannot read queue before recorded restoration: {error}"
                ))
            })?;
        let intended = crate::queue::control::QueueControl {
            paused: false,
            reason: format!(
                "storage reconciliation {transaction} {}",
                if rollback { "rolled back" } else { "activated" }
            ),
            since: Utc::now().to_rfc3339(),
            by: "stado storage-root-reconcile".to_string(),
        };
        let owned_body = owned.intended.to_json();
        let superseding = if current
            .as_ref()
            .is_some_and(|versioned| versioned.content == owned_body)
        {
            None
        } else {
            Some(parse_queue_control(
                current.as_ref().map(|value| value.content.as_str()),
            )?)
        };
        fence.queue.restoration = Some(QueueEffect {
            status: if superseding.is_some() {
                "superseded"
            } else {
                "restore_intent"
            }
            .to_string(),
            expected_version: current.as_ref().map(|value| value.version.clone()),
            expected_content: current.as_ref().map(|value| value.content.clone()),
            intended,
            superseding,
        });
        write_fence(storage_target, transaction, fence, runner).await?;
    }
    let restoration = fence
        .queue
        .restoration
        .as_ref()
        .expect("queue restoration was initialized");
    match restoration.status.as_str() {
        "applied" => {
            fence.queue.resumed = true;
        }
        "superseded" => {
            fence.queue.resumed = false;
        }
        "restore_intent" => match execute_queue_effect(store, restoration).await? {
            QueueEffectOutcome::Applied => {
                fence
                    .queue
                    .restoration
                    .as_mut()
                    .expect("queue restoration was initialized")
                    .status = "applied".to_string();
                fence.queue.resumed = true;
                write_fence(storage_target, transaction, fence, runner).await?;
            }
            QueueEffectOutcome::Superseded(current) => {
                let restoration = fence
                    .queue
                    .restoration
                    .as_mut()
                    .expect("queue restoration was initialized");
                restoration.status = "superseded".to_string();
                restoration.superseding = Some(current);
                fence.queue.resumed = false;
                write_fence(storage_target, transaction, fence, runner).await?;
            }
        },
        status => {
            return Err(DeployError(format!(
                "queue restoration has invalid state {status:?}"
            )));
        }
    }
    Ok(())
}
