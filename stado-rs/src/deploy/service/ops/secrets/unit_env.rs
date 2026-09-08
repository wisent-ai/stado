use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::deploy::service::*;

/// A systemd definition or one drop-in belonging to this declared unit.
pub fn is_systemd_env_file(service: &ManagedService, path: &str) -> bool {
    service.kind == KIND_SYSTEMD
        && !service.path.is_empty()
        && (path == service.path
            || path
                .strip_prefix(&service.path)
                .and_then(|suffix| suffix.strip_prefix(".d/"))
                .is_some_and(|name| {
                    name.ends_with(".conf")
                        && name.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                        })
                }))
}

/// Update a declared systemd environment assignment without cycling the unit.
/// `None` removes the assignment and an otherwise empty drop-in.
pub async fn set_unit_env_key_on_host(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    key: &str,
    value: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !is_systemd_env_file(service, env_path) {
        return Err(DeployError(
            "environment file does not belong to this systemd unit".into(),
        ));
    }
    let body = r###"stado_unit_env_writer() {
  if [ "$scope" = system ]; then
    stado_root "$@"
  elif [ "$service_uid" = "$uid" ]; then
    "$@"
  else
    "$sudo_bin" -n -u "$service_user" "$@"
  fi
}
if ! changed=$(stado_unit_env_writer /usr/bin/python3 - "$service_uid" 2>&1 <<'STADO_UNIT_ENV'
import base64, os, pathlib, re, shlex, stat, sys, tempfile

def decode(value):
    return base64.b64decode(value).decode("utf-8")

raw_path = decode("@ENV_PATH_B64@")
path = pathlib.Path(os.environ["HOME"]) / raw_path[6:] if raw_path.startswith("$HOME/") else pathlib.Path(raw_path)
for component in (path, *path.parents):
    if component.is_symlink():
        raise RuntimeError("unit environment path cannot contain a symlink")
before = path.stat()
if not stat.S_ISREG(before.st_mode) or before.st_uid != int(sys.argv[1]):
    raise RuntimeError("unit environment file must be regular and owned by the service account")
key = decode("@KEY_B64@")
value = decode("@VALUE_B64@") if @SET_VALUE@ else None
original = path.read_text()
entries, pending = [], []
for line in original.splitlines(keepends=True):
    pending.append(line)
    if line.rstrip("\r\n").endswith("\\"):
        continue
    entries.append("".join(pending))
    pending = []
if pending:
    entries.append("".join(pending))

words = re.compile(r"""(?:[^\s"'\\]|\\.|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')+""")
output, in_service, insertion = [], False, None
for raw in entries:
    logical = re.sub(r"\\\r?\n", " ", raw)
    stripped = logical.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        if in_service:
            insertion = len(output)
        in_service = stripped == "[Service]"
    assignment = re.match(r"^\s*Environment\s*=(.*)$", logical.rstrip("\r\n")) if in_service else None
    if assignment and assignment.group(1).strip():
        tokens = words.findall(assignment.group(1))
        retained = [token for token in tokens if shlex.split(token)[0].partition("=")[0] != key]
        if len(retained) != len(tokens):
            if retained:
                output.append("Environment=" + " ".join(retained) + "\n")
            continue
    output.append(raw)
if in_service:
    insertion = len(output)
if value is not None:
    if insertion is None:
        output.append("\n[Service]\n")
        insertion = len(output)
    if insertion and not output[insertion - 1].endswith("\n"):
        output[insertion - 1] += "\n"
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    output.insert(insertion, 'Environment="' + key + "=" + escaped + '"\n')
updated = "".join(output)
empty_dropin = value is None and path.suffix == ".conf" and all(
    not line.strip() or line.strip() == "[Service]" for line in updated.splitlines()
)
if updated == original:
    print("unchanged")
elif empty_dropin:
    current = path.lstat()
    if (current.st_dev, current.st_ino, current.st_mtime_ns, current.st_size) != (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size):
        raise RuntimeError("unit environment file changed during the update")
    path.unlink()
    print("changed")
else:
    fd, temporary = tempfile.mkstemp(prefix=".stado-unit-env.", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            os.fchmod(stream.fileno(), stat.S_IMODE(before.st_mode))
            os.fchown(stream.fileno(), before.st_uid, before.st_gid)
            stream.write(updated)
            stream.flush()
            os.fsync(stream.fileno())
        current = path.lstat()
        if (current.st_dev, current.st_ino, current.st_mtime_ns, current.st_size) != (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size):
            raise RuntimeError("unit environment file changed during the update")
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    print("changed")
STADO_UNIT_ENV
); then
  say '@ACTION@_failed' "$(printf '%s' "$changed" | tr '\t\r\n' '   ')"
  exit 1
fi
if [ "$changed" = changed ] || [ "$(stado_systemctl show -p NeedDaemonReload --value "$unit")" = yes ]; then
  if ! stado_systemctl daemon-reload; then
    say '@ACTION@_failed' 'unit environment changed but systemd could not reload it'
    exit 1
  fi
fi
say '@ACTION@' "$changed; systemd definition refreshed without restarting the unit"
"###;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()))
        .replace(
            "@VALUE_B64@",
            &STANDARD.encode(value.unwrap_or_default().as_bytes()),
        )
        .replace(
            "@SET_VALUE@",
            if value.is_some() { "True" } else { "False" },
        )
        .replace(
            "@ACTION@",
            if value.is_some() {
                "env_set"
            } else {
                "env_unset"
            },
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}
