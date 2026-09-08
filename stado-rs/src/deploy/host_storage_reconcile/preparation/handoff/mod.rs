use super::*;

mod correlate;
mod observe;
mod preflight;
mod queue;

pub(in crate::deploy::host_storage_reconcile) use correlate::*;
pub(in crate::deploy::host_storage_reconcile) use observe::*;
pub(in crate::deploy::host_storage_reconcile) use preflight::*;
pub(in crate::deploy::host_storage_reconcile) use queue::*;

pub(in crate::deploy::host_storage_reconcile) async fn acquire_storage_write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    guard: &mut Option<std::fs::File>,
    runner: &Runner,
) -> Result<(), DeployError> {
    use crate::queue::LocalBackend;
    let roots = fence.roots.as_ref().ok_or_else(|| {
        DeployError("lifecycle fence omitted its observed storage roots".to_string())
    })?;
    let root = PathBuf::from(&roots.primary);
    let paths = LocalBackend::write_fence_paths(&root)
        .ok_or_else(|| DeployError("primary root has no storage write-fence path".to_string()))?;
    if LocalBackend::write_fence_paths(Path::new(&roots.backup)) != Some(paths.clone()) {
        return Err(DeployError(
            "A and B do not share the same storage write fence".to_string(),
        ));
    }
    if fence.write_fence.is_none() {
        fence.write_fence = Some(WriteFenceEffect {
            status: "acquire_intent".to_string(),
            intent: json!({
                "schema": LocalBackend::WRITE_FENCE_PROTOCOL,
                "transaction": transaction,
                "primary_root": roots.primary,
                "backup_root": roots.backup,
                "prepared_at": Utc::now().timestamp(),
            }),
            acquired_at: None,
            released_at: None,
        });
        write_fence(target, transaction, fence, runner).await?;
    }
    let effect = fence
        .write_fence
        .as_ref()
        .expect("write intent was recorded");
    if effect.status == "released" {
        return Ok(());
    }
    if guard.is_none() {
        let file = LocalBackend::open_write_fence_lock(&root)
            .map_err(|error| DeployError(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(DeployError(
                            "in-flight local storage writes did not finish within 30 seconds; \
                             the recorded handoff remains resumable"
                                .to_string(),
                        ));
                    }
                    sleep(Duration::from_millis(25)).await;
                }
                Err(error) => {
                    return Err(DeployError(format!(
                        "cannot acquire storage write fence {}: {error}",
                        paths.0.display()
                    )))
                }
            }
        }
        *guard = Some(file);
    }
    let state =
        LocalBackend::write_fence_state(&root).map_err(|error| DeployError(error.to_string()))?;
    match state.get("intent").filter(|value| !value.is_null()) {
        Some(intent) if intent == &effect.intent => {}
        Some(intent) => {
            return Err(DeployError(format!(
                "storage write fence belongs to a different recorded intent: {intent}"
            )))
        }
        None if effect.status == "acquire_intent" => {
            atomic_json_file(&paths.1, &effect.intent, "storage write-fence intent")?;
        }
        None if effect.status == "release_intent" => return Ok(()),
        None => {
            return Err(DeployError(
                "acquired storage write-fence intent disappeared; refusing to reconstruct it"
                    .to_string(),
            ))
        }
    }
    if fence.write_fence.as_ref().unwrap().status == "acquire_intent" {
        let effect = fence.write_fence.as_mut().unwrap();
        effect.status = "acquired".to_string();
        effect.acquired_at = Some(Utc::now().timestamp());
        write_fence(target, transaction, fence, runner).await?;
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) async fn release_storage_write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    guard: &mut Option<std::fs::File>,
    runner: &Runner,
) -> Result<(), DeployError> {
    use crate::queue::LocalBackend;
    if fence.write_fence.is_none() {
        return Ok(());
    }
    acquire_storage_write_fence(target, transaction, fence, guard, runner).await?;
    if fence.write_fence.as_ref().unwrap().status == "released" {
        return Ok(());
    }
    fence.write_fence.as_mut().unwrap().status = "release_intent".to_string();
    write_fence(target, transaction, fence, runner).await?;
    let root = Path::new(&fence.roots.as_ref().unwrap().primary);
    let (_, intent_path) = LocalBackend::write_fence_paths(root).unwrap();
    let state =
        LocalBackend::write_fence_state(root).map_err(|error| DeployError(error.to_string()))?;
    if let Some(intent) = state.get("intent").filter(|value| !value.is_null()) {
        if intent != &fence.write_fence.as_ref().unwrap().intent {
            return Err(DeployError(
                "storage write-fence intent changed before release".to_string(),
            ));
        }
        std::fs::remove_file(&intent_path).map_err(|error| {
            DeployError(format!("cannot release {}: {error}", intent_path.display()))
        })?;
        std::fs::File::open(intent_path.parent().unwrap())
            .and_then(|directory| directory.sync_all())
            .map_err(|error| DeployError(format!("cannot sync write-fence release: {error}")))?;
    }
    let effect = fence.write_fence.as_mut().unwrap();
    effect.status = "released".to_string();
    effect.released_at = Some(Utc::now().timestamp());
    write_fence(target, transaction, fence, runner).await?;
    *guard = None;
    Ok(())
}
