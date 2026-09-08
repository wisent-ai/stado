use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn prove_listener_closed(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<(), DeployError> {
    let script = format!(
        r#"PORT={} /usr/bin/python3 - <<'PY'
import os, socket, time
port = int(os.environ['PORT'])
deadline = time.monotonic() + 30
while True:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.settimeout(0.2)
    result = probe.connect_ex(('127.0.0.1', port))
    probe.close()
    if result != 0:
        print('STADO_LISTENER_CLOSED\t' + str(port))
        break
    if time.monotonic() >= deadline:
        raise SystemExit('object API listener remained open')
    time.sleep(0.2)
PY"#,
        port
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_LISTENER_CLOSED\t{port}");
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(format!(
            "object API listener on {}:{port} did not close: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) async fn snapshot_unit_file(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<FileSnapshot>, DeployError> {
    let script = format!(
        r#"STADO_UNIT_PATH={} /usr/bin/python3 - <<'PY'
import base64, hashlib, json, os, stat
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))
try:
    info = os.lstat(path)
except FileNotFoundError:
    print('STADO_UNIT_SNAPSHOT\tabsent')
    raise SystemExit(0)
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
    raise SystemExit('unit path is not a regular non-symlink file')
with open(path, 'rb') as handle:
    body = handle.read()
print('STADO_UNIT_SNAPSHOT\t' + json.dumps({{
    'body_base64': base64.b64encode(body).decode('ascii'),
    'sha256': hashlib.sha256(body).hexdigest(),
    'mode': stat.S_IMODE(info.st_mode),
    'uid': info.st_uid,
    'gid': info.st_gid,
}}, sort_keys=True, separators=(',', ':')))
PY"#,
        shlex_quote(path)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "unit snapshot failed for {path} on {}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    let value = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNIT_SNAPSHOT\t"))
        .ok_or_else(|| DeployError("unit snapshot returned no marker".to_string()))?;
    if value == "absent" {
        return Ok(None);
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| DeployError(format!("unit snapshot is invalid: {error}")))
}
pub(in crate::deploy::host_storage_reconcile) fn unit_declared_environment(
    candidate: &ServiceCandidate,
    snapshot: Option<&FileSnapshot>,
) -> Result<BTreeMap<String, String>, DeployError> {
    let Some(snapshot) = snapshot else {
        return Ok(BTreeMap::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&snapshot.body_base64)
        .map_err(|error| DeployError(format!("unit snapshot base64 is invalid: {error}")))?;
    let content = String::from_utf8(bytes)
        .map_err(|error| DeployError(format!("unit snapshot is not UTF-8: {error}")))?;
    let kind = if candidate.declared.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    let unit = service::UnitFile {
        host: candidate.target.name.clone(),
        unit: candidate.declared.unit_id().to_string(),
        path: candidate.declared.path.clone(),
        kind,
        content,
    };
    let parsed = service::unit_environment(&unit)?;
    Ok(parsed.env.into_iter().collect())
}
