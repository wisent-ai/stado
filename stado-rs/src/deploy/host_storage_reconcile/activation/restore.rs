use super::*;

pub(super) async fn restore_unit_snapshot(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let snapshot = writer
        .unit_snapshot
        .as_ref()
        .ok_or_else(|| DeployError(format!("{} has no captured exact unit bytes", writer.label)))?;
    let script = format!(
        r#"STADO_UNIT_PATH={} STADO_UNIT_BODY={} STADO_UNIT_SHA={} STADO_UNIT_MODE={} STADO_UNIT_UID={} STADO_UNIT_GID={} /usr/bin/python3 - <<'PY'
import base64, hashlib, os, stat, subprocess, tempfile
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))
body = base64.b64decode(os.environ['STADO_UNIT_BODY'])
expected = os.environ['STADO_UNIT_SHA']
if hashlib.sha256(body).hexdigest() != expected:
    raise SystemExit('captured unit bytes fail their digest')
expected_metadata = (int(os.environ['STADO_UNIT_MODE']),
                     int(os.environ['STADO_UNIT_UID']),
                     int(os.environ['STADO_UNIT_GID']))
work = os.path.expanduser('~/.stado/work/storage-root-reconcile-units')
os.makedirs(work, mode=0o700, exist_ok=True)
fd, temporary = tempfile.mkstemp(prefix='unit.', dir=work)
try:
    with os.fdopen(fd, 'wb') as handle:
        handle.write(body)
        handle.flush()
        os.fsync(handle.fileno())
    command = ['/usr/bin/sudo', '-n', '/usr/bin/install',
               '-m', format(expected_metadata[0], 'o'),
               '-o', os.environ['STADO_UNIT_UID'],
               '-g', os.environ['STADO_UNIT_GID'], temporary, path]
    result = subprocess.run(command, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, close_fds=False)
    if result.returncode != 0:
        raise SystemExit((result.stderr or result.stdout).strip())
finally:
    try:
        os.unlink(temporary)
    except FileNotFoundError:
        pass
info = os.lstat(path)
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
    raise SystemExit('restored unit is not a regular file')
observed_metadata = (stat.S_IMODE(info.st_mode), info.st_uid, info.st_gid)
if observed_metadata != expected_metadata:
    raise SystemExit('restored unit mode/uid/gid mismatch: expected ' +
                     str(expected_metadata) + ', observed ' + str(observed_metadata))
with open(path, 'rb') as handle:
    if hashlib.sha256(handle.read()).hexdigest() != expected:
        raise SystemExit('restored unit digest mismatch')
print('STADO_UNIT_RESTORED\t' + expected)
PY"#,
        shlex_quote(&writer.path),
        shlex_quote(&snapshot.body_base64),
        shlex_quote(&snapshot.sha256),
        snapshot.mode,
        snapshot.uid,
        snapshot.gid,
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_UNIT_RESTORED\t{}", snapshot.sha256);
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(format!(
            "exact unit restoration failed for {} on {}: {}",
            writer.label,
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

pub(super) fn restored_state_matches(
    writer: &WriterFence,
    state: &crate::deploy::service_label_print::LabelState,
    autostart: &BTreeMap<String, bool>,
    active_sha256: &str,
    roots: &StorageRoots,
    rollback: bool,
) -> bool {
    if autostart != &writer.autostart {
        return false;
    }
    let should_be_loaded = writer.was_loaded || writer.was_runnable;
    if state.loaded() != should_be_loaded {
        return false;
    }
    if !should_be_loaded {
        return state.pid.is_none();
    }
    if let Some(pid) = state.pid.as_deref() {
        if pid == "0"
            || state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none_or(|inode| inode == 0)
        {
            return false;
        }
        let expected_sha256 = if writer.role == "object-api"
            || writer
                .prior_executable
                .as_deref()
                .is_some_and(|path| executable_name(path) == "stado")
        {
            Some(active_sha256)
        } else {
            writer.prior_sha256.as_deref()
        };
        if state.process_sha256.as_deref() != expected_sha256 {
            return false;
        }
    } else if writer.role == "object-api"
        || (state.state.is_none()
            && state.last_exit_code.is_none()
            && state.restart.is_none()
            && state.triggers.is_none())
    {
        return false;
    }
    if writer.role != "object-api" {
        return true;
    }
    let loaded = &state.loaded_environment;
    if !loaded.contains_key("WC_STORAGE_BACKEND") {
        return true;
    }
    let expected_config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .map(String::as_str);
    if loaded.get("WC_STORAGE_BACKEND").map(String::as_str) != Some("local")
        || loaded.get("STADO_CONFIG").map(String::as_str) != expected_config
    {
        return false;
    }
    let (primary, backup) = if rollback {
        (roots.prior_primary.as_str(), roots.prior_backup.as_deref())
    } else {
        (roots.primary.as_str(), Some(roots.backup.as_str()))
    };
    loaded.get("WC_LOCAL_STORAGE_PATH").map(String::as_str) == Some(primary)
        && loaded
            .get("WC_BACKUP_STORAGE_BACKEND")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup.map(|_| "local")
        && loaded
            .get("WC_BACKUP_LOCAL_STORAGE_PATH")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup
}

pub(super) fn durable_restored_state_matches(
    writer: &WriterFence,
    state: &crate::deploy::service_label_print::LabelState,
) -> bool {
    (writer.role != "object-api" || writer.restored_route.is_some())
        && state.pid == writer.restored_pid
        && state.process_started_at == writer.restored_started_at
        && state.loaded_environment == writer.restored_loaded_environment
        && state.process_executable == writer.restored_executable
        && state.process_sha256 == writer.restored_sha256
        && state.process_device == writer.restored_device
        && state.process_inode == writer.restored_inode
}
