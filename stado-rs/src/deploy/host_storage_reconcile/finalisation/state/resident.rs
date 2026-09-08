use super::*;

pub(in crate::deploy::host_storage_reconcile) fn resident_owner_retention(
    transaction: &str,
) -> Result<Value, DeployError> {
    verify_resident_lock(transaction)?;
    let identity = RESIDENT_NATIVE_MANAGER.get().ok_or_else(|| {
        DeployError("resident native manager identity was not initialized".to_string())
    })?;
    let expected_service = if cfg!(target_os = "linux") {
        format!("com.wisent.stado-storage-root-reconcile.{transaction}.service")
    } else {
        format!("com.wisent.stado-storage-root-reconcile.{transaction}")
    };
    if identity.get("service").and_then(Value::as_str) != Some(expected_service.as_str())
        || identity.get("pid").and_then(Value::as_u64) != Some(u64::from(std::process::id()))
    {
        return Err(DeployError(
            "resident native manager identity does not bind this exact transaction process"
                .to_string(),
        ));
    }
    let lock_path = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| DeployError("transaction directory has no authority root".to_string()))?
        .join("storage-root-reconcile.lock");
    let lock = std::fs::metadata(&lock_path)
        .map_err(|error| DeployError(format!("cannot inspect resident lock identity: {error}")))?;
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
    })?;
    let lock = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    let path_metadata = std::fs::metadata(&lock)
        .map_err(|error| DeployError(format!("cannot stat {}: {error}", lock.display())))?;
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
        })?;
    if path_metadata.dev() as nix::libc::dev_t != descriptor_metadata.st_dev
        || path_metadata.ino() != descriptor_metadata.st_ino
    {
        return Err(DeployError(
            "resident reconciliation lock no longer maps the canonical transaction lock"
                .to_string(),
        ));
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) fn resident_native_manager_identity(
    transaction: &str,
) -> Result<Value, DeployError> {
    let label = format!("com.wisent.stado-storage-root-reconcile.{transaction}");
    let current_pid = std::process::id();
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/launchctl", "print", &format!("system/{label}")])
            .output()
            .map_err(|error| {
                DeployError(format!("cannot query resident launchd owner: {error}"))
            })?;
        if !output.status.success() {
            return Err(DeployError(
                "resident worker is not loaded in its captured launchd service".to_string(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let pid = stdout.lines().find_map(|line| {
            line.trim()
                .strip_prefix("pid = ")
                .and_then(|value| value.parse::<u32>().ok())
        });
        let state = stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("state = ").map(str::to_string));
        if pid != Some(current_pid) {
            return Err(DeployError(format!(
                "launchd binds the resident service to pid {pid:?}, not worker pid {current_pid}"
            )));
        }
        return Ok(json!({
            "manager": "launchd",
            "service": label,
            "domain": "system",
            "pid": current_pid,
            "state": state,
        }));
    }
    if cfg!(target_os = "linux") {
        let unit = format!("{label}.service");
        let output = std::process::Command::new("/usr/bin/sudo")
            .args([
                "-n",
                "/bin/systemctl",
                "show",
                "--property=LoadState,ActiveState,SubState,MainPID",
                &unit,
            ])
            .output()
            .map_err(|error| {
                DeployError(format!("cannot query resident systemd owner: {error}"))
            })?;
        if !output.status.success() {
            return Err(DeployError(
                "resident worker is not loaded in its captured systemd service".to_string(),
            ));
        }
        let properties = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>();
        let pid = properties
            .get("MainPID")
            .and_then(|value| value.parse::<u32>().ok());
        if properties.get("LoadState").map(String::as_str) != Some("loaded")
            || !matches!(
                properties.get("ActiveState").map(String::as_str),
                Some("active" | "activating" | "reloading")
            )
            || pid != Some(current_pid)
        {
            return Err(DeployError(format!(
                "systemd does not bind {} to worker pid {}: {:?}",
                unit, current_pid, properties
            )));
        }
        return Ok(json!({
            "manager": "systemd",
            "service": unit,
            "pid": current_pid,
            "load_state": properties.get("LoadState"),
            "active_state": properties.get("ActiveState"),
            "sub_state": properties.get("SubState"),
        }));
    }
    Err(DeployError(
        "native reconciliation worker requires Darwin launchd or Linux systemd".to_string(),
    ))
}
