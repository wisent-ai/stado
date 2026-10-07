use super::*;

pub(in crate::deploy::host_storage_reconcile) fn resident_owner_retention(
    transaction: &str,
) -> Result<Value, DeployError> {
    verify_resident_lock(transaction)?;
    let identity = RESIDENT_NATIVE_MANAGER.get().ok_or_else(|| {
        DeployError("resident native manager identity was not initialized".to_string())
    })?;
    let expected_service = crate::deploy::local_install::stado_unit()?;
    if identity.get("service").and_then(Value::as_str) != Some(expected_service.as_str())
        || identity.get("pid").and_then(Value::as_u64) != Some(u64::from(std::process::id()))
    {
        return Err(DeployError(
            "resident native manager identity does not bind this exact transaction process"
                .to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let lock_path = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            DeployError("transaction directory has no authority root".to_string())
                .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .join("storage-root-reconcile.lock");
    let lock = std::fs::metadata(&lock_path).map_err(DeployError::io(
        "cannot inspect resident lock identity".to_string(),
    ))?;
    Ok(json!({
        "role": "resident-transaction-owner",
        "native_manager": identity,
        "process_pid": std::process::id(),
        "lock_device": lock.dev(),
        "lock_inode": lock.ino(),
    }))
}

pub(in crate::deploy::host_storage_reconcile) fn verify_resident_lock(
    transaction: &str,
) -> Result<(), DeployError> {
    let fd = RESIDENT_LOCK_FD.get().copied().ok_or_else(|| {
        DeployError("resident reconciliation lock descriptor is absent".to_string())
            .stating(crate::primitives::failure::FailureCode::Refused)
    })?;
    let lock = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    let path_metadata = std::fs::metadata(&lock)
        .map_err(DeployError::io(format!("cannot stat {}", lock.display())))?;
    // Ask the descriptor itself. Darwin's fdesc filesystem does not promise
    // that statting `/dev/fd/N` exposes the opened object's device and inode;
    // the descriptor-authoritative `fstat(2)` does on every supported host.
    // SAFETY: the worker-owned `operation_lock` remains alive until after the
    // reconciliation outcome is recorded.
    let descriptor_metadata = nix::sys::stat::fstat(unsafe { BorrowedFd::borrow_raw(fd) })
        .map_err(|error| {
            DeployError(format!(
                "resident reconciliation lock descriptor {fd} is invalid: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(
                std::io::Error::from(error).kind(),
            ))
        })?;
    if path_metadata.dev() as nix::libc::dev_t != descriptor_metadata.st_dev
        || path_metadata.ino() != descriptor_metadata.st_ino
    {
        return Err(DeployError(
            "resident reconciliation lock no longer maps the canonical transaction lock"
                .to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    Ok(())
}

/// The worker's manager is the host's one Stado process, which runs this
/// worker as its child: the identity it records is that process's unit, the
/// worker's own pid as that process reports it, and the host process's pid.
pub(in crate::deploy::host_storage_reconcile) fn resident_native_manager_identity(
    transaction: &str,
) -> Result<Value, DeployError> {
    let service = crate::deploy::local_install::stado_unit()?;
    let current_pid = std::process::id();
    let Some((host_pid, worker)) =
        crate::release_agent::rollout::serving::control::inspect_transaction_blocking(
            None,
            transaction,
        )
        .map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within("cannot query the host process")
        })?
    else {
        return Err(DeployError::unreachable(
            "the host process is not running, so nothing manages this worker".to_string(),
        ));
    };
    let worker = worker.ok_or_else(|| {
        DeployError(format!(
            "the host process (pid {host_pid}) runs no worker of transaction {transaction}"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    if worker.pid != current_pid {
        return Err(DeployError(format!(
            "the host process binds transaction {transaction} to pid {}, not worker pid \
             {current_pid}",
            worker.pid
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    Ok(json!({
        "manager": "stado",
        "service": service,
        "pid": current_pid,
        "host_pid": host_pid,
        "state": worker.exit.unwrap_or_else(|| "running".to_string()),
    }))
}
