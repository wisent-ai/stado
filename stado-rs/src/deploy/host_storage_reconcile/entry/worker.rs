use super::*;

pub async fn reconcile_host_worker(
    target: crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    source_revision: &str,
    tool_sha256: &str,
    runner_gate: Option<Value>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    use fs2::FileExt;

    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "resident worker action must be {RUN}, {RESUME}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    if !host_channel::target_is_this_host(&target) {
        return Err(DeployError(
            "native reconciliation worker is not resident on its captured target".to_string(),
        ));
    }
    if source_revision != crate::binary::build_identity::SOURCE_REVISION
        || source_revision == crate::binary::build_identity::UNKNOWN_REVISION
        || source_revision.ends_with("-dirty")
    {
        return Err(DeployError(
            "resident transaction tool does not carry one clean exact source revision".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let actual_sha256 = sha256_file(&executable)?;
    if actual_sha256 != tool_sha256 {
        return Err(DeployError(
            "resident transaction tool digest differs from launch request".to_string(),
        ));
    }
    let directory = transaction_directory(transaction)?;
    let lock_path = directory
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    }
    let operation_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|error| DeployError(format!("cannot open native transaction lock: {error}")))?;
    operation_lock.try_lock_exclusive().map_err(|error| {
        DeployError(format!(
            "another reconciliation owns the native lock: {error}"
        ))
    })?;
    let descriptor = operation_lock.as_raw_fd();
    // SAFETY: `descriptor` is owned by `operation_lock`; F_GETFD/F_SETFD do
    // not consume it. Clearing CLOEXEC deliberately carries the same locked
    // open-file description through every locally spawned lifecycle effect.
    let flags = unsafe { nix::libc::fcntl(descriptor, nix::libc::F_GETFD) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                descriptor,
                nix::libc::F_SETFD,
                flags & !nix::libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(DeployError(format!(
            "cannot make native transaction lock inheritable: {}",
            std::io::Error::last_os_error()
        )));
    }
    let lock_metadata = std::fs::metadata(&lock_path)
        .map_err(|error| DeployError(format!("cannot stat native lock path: {error}")))?;
    let descriptor_metadata = operation_lock
        .metadata()
        .map_err(|error| DeployError(format!("cannot stat native lock descriptor: {error}")))?;
    if lock_metadata.dev() != descriptor_metadata.dev()
        || lock_metadata.ino() != descriptor_metadata.ino()
    {
        return Err(DeployError(
            "opened descriptor is not the canonical reconciliation lock".to_string(),
        ));
    }
    RESIDENT_LOCK_FD
        .set(descriptor)
        .map_err(|_| DeployError("resident lock descriptor was already initialized".to_string()))?;
    RESIDENT_TARGET
        .set(target.clone())
        .map_err(|_| DeployError("resident target was already initialized".to_string()))?;
    let token = uuid::Uuid::new_v4().to_string();
    RESIDENT_OWNER_TOKEN
        .set(token.clone())
        .map_err(|_| DeployError("resident owner token was already initialized".to_string()))?;
    if let Some(gate) = runner_gate {
        RESIDENT_RUNNER_GATE
            .set(gate)
            .map_err(|_| DeployError("resident runner gate was already initialized".to_string()))?;
    }
    let native_manager = resident_native_manager_identity(transaction)?;
    RESIDENT_NATIVE_MANAGER
        .set(native_manager.clone())
        .map_err(|_| {
            DeployError("resident native manager identity was already initialized".to_string())
        })?;
    let owner_path = directory.join("operation-owner.json");
    let revision = std::fs::read(&owner_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|owner| owner.get("revision").and_then(Value::as_u64))
        .unwrap_or_default()
        .saturating_add(1);
    let mut owner = json!({
        "schema": "stado.storage-root-owner.v1",
        "transaction": transaction,
        "target": target.name.clone(),
        "action": phase,
        "status": "executing",
        "pid": std::process::id(),
        "token": token,
        "source_revision": source_revision,
        "tool_path": executable,
        "tool_sha256": actual_sha256,
        "lock_device": descriptor_metadata.dev(),
        "lock_inode": descriptor_metadata.ino(),
        "native_manager": native_manager,
        "target_config": serde_json::to_value(&target)
            .map_err(|error| DeployError(format!("cannot capture resident target: {error}")))?,
        "revision": revision,
        "started_at": Utc::now().to_rfc3339(),
        "updated_at": Utc::now().to_rfc3339(),
    });
    atomic_owner(&owner_path, &owner)?;
    let outcome = reconcile_host_inner(&target, transaction, phase, runner).await;
    let fields = owner
        .as_object_mut()
        .expect("resident operation owner is an object");
    fields.insert("updated_at".to_string(), json!(Utc::now().to_rfc3339()));
    match &outcome {
        Ok(result) => {
            fields.insert("status".to_string(), json!("succeeded"));
            fields.insert("result".to_string(), result.clone());
        }
        Err(error) => {
            fields.insert("status".to_string(), json!("failed"));
            fields.insert("error".to_string(), json!(error.to_string()));
        }
    }
    atomic_owner(&owner_path, &owner)?;
    drop(operation_lock);
    outcome
}
