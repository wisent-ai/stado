use super::*;

pub(in crate::deploy::host_storage_reconcile) fn prepared_script(body: String) -> PreparedScript {
    PreparedScript {
        sha256: hex::encode(Sha256::digest(body.as_bytes())),
        body,
    }
}
pub(in crate::deploy::host_storage_reconcile) async fn correlate_served_store(
    target: &crate::targets::ComputeTarget,
    port: u16,
    preflight: &Value,
    primary_after_commit: bool,
    conflict_winner: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !matches!(conflict_winner, "primary" | "backup") {
        return Err(DeployError(
            "served-store correlation conflict winner is invalid".to_string(),
        ));
    }
    let payload = serde_json::to_vec(&json!({
        "primary": preflight.get("primary_qualified"),
        "backup": preflight.get("backup_qualified"),
        "primary_physical": preflight.get("primary_physical"),
        "backup_physical": preflight.get("backup_physical"),
        "primary_after_commit": primary_after_commit,
        "conflict_winner": conflict_winner,
    }))
    .map_err(|error| DeployError(format!("cannot encode served-store inventory: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let script = format!(
        r#"STADO_OBJECT_PORT={port} /usr/bin/python3 - <<'PY'
import base64, hashlib, json, os, urllib.parse, urllib.request
payload = json.loads(base64.b64decode('{encoded}'))
port = int(os.environ['STADO_OBJECT_PORT'])
token_path = os.path.expanduser('~/.stado/queue-object-api-token')
with open(token_path, encoding='utf-8') as handle:
    token = handle.read().strip()
if not token:
    raise SystemExit('object API correlation token is empty')
headers = {{'Authorization': 'Bearer ' + token}}
base = 'http://127.0.0.1:' + str(port)
request = urllib.request.Request(base + '/api/object/list?namespace=probierz&prefix=', headers=headers)
with urllib.request.urlopen(request, timeout=30) as response:
    listed = json.load(response)
keys = sorted(item.get('key') for item in listed.get('objects', []) if isinstance(item.get('key'), str))
def identities(name):
    result = {{}}
    prefix = 'ecosystem/probierz/'
    for item in payload[name]:
        path = item.get('path', '')
        if not path.startswith(prefix):
            continue
        result[path[len(prefix):]] = item.get('body')
    return result
primary_before = identities('primary')
backup = identities('backup')
primary = dict(primary_before)
if payload.get('primary_after_commit'):
    if payload.get('conflict_winner') == 'primary':
        primary = dict(backup)
        primary.update(primary_before)
    else:
        primary.update(backup)
served = {{}}
for key in keys:
    uri = 'stado://probierz/' + key
    url = base + '/api/object?uri=' + urllib.parse.quote(uri, safe='')
    request = urllib.request.Request(url, headers=headers)
    digest = hashlib.sha256()
    size = 0
    with urllib.request.urlopen(request, timeout=60) as response:
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
    served[key] = {{'sha256': digest.hexdigest(), 'bytes': size}}
matches_primary = keys == sorted(primary) and all(served[key] == primary[key] for key in keys)
matches_backup = keys == sorted(backup) and all(served[key] == backup[key] for key in keys)
if not matches_primary and not matches_backup:
    raise SystemExit('object API does not serve either complete physical qualified root')
authority = 'identical' if matches_primary and matches_backup else 'A' if matches_primary else 'B'
def physical_identity(name, path):
    for item in payload[name].get('files', []):
        if item.get('path') == path:
            return item.get('body')
    return None
object_mappings = [{{
    'backend': 'stado-object-api', 'namespace': 'probierz', 'key': key,
    'physical_path': 'ecosystem/probierz/' + key, 'identity': served[key],
}} for key in keys]
registry_mappings = [
    {{'root': 'A', 'backend': 'local', 'namespace': None, 'key': 'registry.json',
      'physical_path': 'registry.json',
      'identity': physical_identity('primary_physical', 'registry.json')}},
    {{'root': 'B', 'backend': 'local', 'namespace': None, 'key': 'registry.json',
      'physical_path': 'registry.json',
      'identity': physical_identity('backup_physical', 'registry.json')}},
    {{'root': 'served', 'backend': 'stado-object', 'namespace': None,
      'key': 'registry.json', 'physical_path': None,
      'observation': 'client namespace was not observable from the object API'}},
]
print('STADO_SERVED_STORE\t' + json.dumps({{
    'object_authority': authority,
    'endpoint': base,
    'object_store': {{'backend': 'stado-object-api', 'namespace': 'probierz',
                     'objects': object_mappings}},
    'registry_store': {{'mappings': registry_mappings}},
    'primary_root': os.path.expanduser('~/.stado/local-storage'),
    'backup_root': os.path.expanduser('~/.stado/local-backup'),
}}, sort_keys=True, separators=(',', ':')))
PY"#
    );
    let output = host_channel::run_script_with_timeout(target, &script, TIMEOUT, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "object API physical-store correlation failed on {}:{port}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_SERVED_STORE\t"))
        .ok_or_else(|| DeployError("object API correlation returned no evidence".to_string()))
        .and_then(|body| {
            serde_json::from_str(body)
                .map_err(|error| DeployError(format!("object API correlation is invalid: {error}")))
        })
}
pub(in crate::deploy::host_storage_reconcile) async fn observe_object_runtime(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let script = format!(
        r#"python3 - <<'PY'
import json, urllib.request
with urllib.request.urlopen('http://127.0.0.1:{port}/api/state.json', timeout=30) as response:
    state = json.load(response)
print('STADO_STORAGE_RECONCILE\t' + json.dumps(state, sort_keys=True))
PY
"#
    );
    let output = host_channel::run_script_with_timeout(target, &script, TIMEOUT, runner).await?;
    parse_remote_payload(&output)
}
