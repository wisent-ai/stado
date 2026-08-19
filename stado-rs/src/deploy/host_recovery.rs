//! Registry-authorized recovery for a managed macOS host.
//!
//! The remote program is fixed and deliberately narrow: run the canonical
//! Rust disk cleanup, disable the obsolete local coordinator, and reload the
//! one registry-managed health agent after validating its scoped Skarbiec
//! configuration. Registry data selects only the host; it cannot supply shell
//! fragments.
//!
//! The tab-delimited `STADO_*` marker protocol (script emission in
//! [`remote_script`], parsing in [`parse_output`]) deliberately preserves the
//! mix of literal `\t` / `\n` escape sequences and real control characters
//! consumed by the recovery report parser.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{py_str_repr, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::{normalize_hostname, ssh_hostname, ComputeTarget, Registry};

/// Python `_TIMEOUT_SECONDS`.
pub const TIMEOUT_SECONDS: u64 = 120;

/// Rust Stado cleanup binary. Recovery has no Python-package fallback.
pub const WC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/stado"];

/// LaunchAgents whose recovery remains host-scoped. Weles lifecycle is owned
/// exclusively by the authenticated Stado service API.
pub const MANAGED_AGENTS: &[(&str, &str)] = &[(
    "com.wisent.host-health-beacon",
    "$HOME/Library/LaunchAgents/com.wisent.host-health-beacon.plist",
)];

/// The fixed remote program with `@IDENTITY_WORDS@` / `@WC_WORDS@` /
/// `@AGENT_ROWS@` substitution points. Written with explicit escapes so
/// `\t` / `\r` / `\n` are real control characters while `\\t` / `\\n`
/// remain literal backslash sequences where the marker protocol requires
/// them.
const REMOTE_SCRIPT_TEMPLATE: &str = "set -u
host=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')
identity_ok=0
for expected in @IDENTITY_WORDS@; do
  short=\"${expected%.local}\"
  if [ \"$host\" = \"$expected\" ] || [ \"$host\" = \"$short\" ]; then identity_ok=1; fi
done
if [ \"$identity_ok\" -ne 1 ]; then
  printf 'STADO_RECOVER\\tidentity_mismatch\\t%s\\n' \"$host\"
  exit 64
fi
if [ \"$(/usr/bin/uname -s)\" != \"Darwin\" ]; then
  printf 'STADO_RECOVER\\tunsupported_os\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 65
fi

disk_before=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
wc_bin=\"\"
for candidate in @WC_WORDS@; do
  if [ -x \"$candidate\" ]; then wc_bin=\"$candidate\"; break; fi
done
cleanup_status=\"unavailable\"
cleanup_json=\"\"
if [ -n \"$wc_bin\" ]; then
  cleanup_json=$(\"$wc_bin\" disk-cleanup --once)
  cleanup_rc=$?
  if [ \"$cleanup_rc\" -eq 0 ]; then cleanup_status=\"ok\"; else cleanup_status=\"failed:$cleanup_rc\"; fi
fi

uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
if /bin/launchctl print \"$gui\" >/dev/null 2>&1; then
  agent_domain=\"$gui\"
  printf 'STADO_DOMAIN\t%s\tavailable\n' \"$agent_domain\"
elif /bin/launchctl print \"$user_domain\" >/dev/null 2>&1; then
  agent_domain=\"$user_domain\"
  printf 'STADO_DOMAIN\t%s\tfallback\n' \"$agent_domain\"
else
  printf 'STADO_DOMAIN\t%s\tunavailable\n' \"$gui\"
  exit 66
fi
/bin/launchctl bootout \"$gui/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl bootout \"$user_domain/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl disable \"$gui/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl disable \"$user_domain/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true

recover_agent() {
  label=\"$1\"
  plist=\"$2\"
  if [ ! -f \"$plist\" ]; then
    printf 'STADO_AGENT\\t%s\\tmissing_plist\\n' \"$label\"
    return
  fi
  if [ \"$label\" = \"com.wisent.host-health-beacon\" ]; then
    api_url=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_API_URL raw -o - \"$plist\" || true)
    vault_url=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_URL raw -o - \"$plist\" || true)
    consumer=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_CONSUMER raw -o - \"$plist\" || true)
    grant_file=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE raw -o - \"$plist\" || true)
    stado_bin=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_BIN raw -o - \"$plist\" || true)
    if [ -z \"$api_url\" ] || [ -z \"$vault_url\" ] || [ -z \"$grant_file\" ] || [ -z \"$stado_bin\" ] || [ \"$consumer\" != \"stado-host-health-beacon\" ]; then
      printf 'STADO_AGENT\\t%s\\tinvalid_scoped_health_config\\n' \"$label\"
      return
    fi
    if /usr/bin/plutil -extract EnvironmentVariables.GOOGLE_APPLICATION_CREDENTIALS raw -o - \"$plist\" >/dev/null || /usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_API_TOKEN raw -o - \"$plist\" >/dev/null; then
      printf 'STADO_AGENT\\t%s\\tforbidden_ambient_health_credential\\n' \"$label\"
      return
    fi
  fi
  /bin/launchctl bootout \"$gui/$label\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$label\" >/dev/null 2>&1 || true
  bootstrap_detail=$(/bin/launchctl bootstrap \"$agent_domain\" \"$plist\" 2>&1)
  bootstrap_rc=$?
  if [ \"$bootstrap_rc\" -eq 0 ]; then
    /bin/launchctl enable \"$agent_domain/$label\" >/dev/null 2>&1 || true
    /bin/launchctl kickstart -k \"$agent_domain/$label\" >/dev/null 2>&1 || true
    printf 'STADO_AGENT\t%s\trestarted\n' \"$label\"
  else
    bootstrap_detail=$(printf '%s' \"$bootstrap_detail\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
    printf 'STADO_AGENT\t%s\tbootstrap_failed:%s:%s\n' \"$label\" \"$bootstrap_rc\" \"$bootstrap_detail\"
  fi
}

@AGENT_ROWS@
/bin/sleep 5
disk_after=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
printf 'STADO_RECOVER\\tok\\t%s\\t%s\\t%s\\t%s\\n' \"$host\" \"${disk_before:-0}\" \"${disk_after:-0}\" \"$cleanup_status\"
if [ -n \"$cleanup_json\" ]; then printf 'STADO_CLEANUP\\t%s\\n' \"$cleanup_json\"; fi
";

/// Python `_identity_values`: normalized names, hostname aliases, and the
/// host part of the SSH destination; empty values dropped, sorted.
pub fn identity_values(target: &ComputeTarget) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    values.push(normalize_hostname(&target.name));
    values.extend(target.hostnames.iter().map(|v| normalize_hostname(v)));
    if let Some(ssh) = &target.ssh {
        values.push(ssh_hostname(ssh));
    }
    values.retain(|v| !v.is_empty());
    values.sort();
    values.dedup();
    values
}

/// Python `_remote_script`: the fixed recovery program with this target's
/// identity words spliced in.
pub fn remote_script(target: &ComputeTarget) -> String {
    let identity_words = identity_values(target)
        .iter()
        .map(|value| shlex_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let agent_rows = MANAGED_AGENTS
        .iter()
        .map(|(label, plist)| format!("recover_agent {} \"{plist}\"", shlex_quote(label)))
        .collect::<Vec<_>>()
        .join("\n");
    REMOTE_SCRIPT_TEMPLATE
        .replace("@IDENTITY_WORDS@", &identity_words)
        .replace("@WC_WORDS@", &wc_words)
        .replace("@AGENT_ROWS@", &agent_rows)
        .replace("/usr/bin/tr '\t\r\n' ' '", r"/usr/bin/tr '\t\r\n' ' '")
}

/// Python `recover_host` ssh argv (note the -o order: BatchMode,
/// ConnectTimeout, StrictHostKeyChecking).
pub fn ssh_argv(ssh_target: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        ssh_target.to_string(),
        "/bin/bash".to_string(),
        "-s".to_string(),
    ]
}

/// Python `_parse_output`: fold the `STADO_*` marker lines of stdout into
/// the report dict. `Err` on a non-integer disk field (Python's
/// `int(fields[3])` raising ValueError).
pub fn parse_output(stdout: &str, target: &ComputeTarget) -> Result<Value, DeployError> {
    let mut report = Map::new();
    report.insert("target".to_string(), json!(target.name));
    report.insert(
        "ssh".to_string(),
        target.ssh.as_ref().map_or(Value::Null, |ssh| json!(ssh)),
    );
    report.insert("status".to_string(), json!("failed"));
    report.insert("agents".to_string(), json!(Map::new()));
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.first() == Some(&"STADO_AGENT") && fields.len() == 3 {
            report["agents"][fields[1]] = json!(fields[2]);
        } else if fields.first() == Some(&"STADO_DOMAIN") && fields.len() == 3 {
            report.insert(
                "launchd_domain".to_string(),
                json!({"name": fields[1], "status": fields[2]}),
            );
        } else if fields.first() == Some(&"STADO_RECOVER")
            && fields.get(1) == Some(&"ok")
            && fields.len() == 6
        {
            let before = parse_int_field(fields[3])?;
            let after = parse_int_field(fields[4])?;
            report.insert("status".to_string(), json!("ok"));
            report.insert("host".to_string(), json!(fields[2]));
            report.insert("disk_free_kb_before".to_string(), json!(before));
            report.insert("disk_free_kb_after".to_string(), json!(after));
            report.insert("cleanup_status".to_string(), json!(fields[5]));
        } else if fields.first() == Some(&"STADO_CLEANUP") && fields.len() == 2 {
            let cleanup = serde_json::from_str(fields[1])
                .unwrap_or_else(|_| json!({"outcome": "invalid_output"}));
            report.insert("cleanup".to_string(), cleanup);
        } else if fields.first() == Some(&"STADO_RECOVER") {
            report.insert(
                "remote_error".to_string(),
                json!(fields[1..]
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()),
            );
        }
    }
    Ok(Value::Object(report))
}

/// Python `int(fields[i])` with the CPython ValueError message.
fn parse_int_field(field: &str) -> Result<i64, DeployError> {
    field.parse::<i64>().map_err(|_| {
        DeployError(format!(
            "invalid literal for int() with base 10: {}",
            py_str_repr(field)
        ))
    })
}

/// Python `_target`: resolve a canonical kind=local registry host.
fn resolve_target<'a>(
    registry: &'a Registry,
    target_name: &str,
) -> Result<&'a ComputeTarget, DeployError> {
    let Some(target) = registry.lookup(target_name) else {
        return Err(DeployError(format!(
            "target {} is not in the canonical registry",
            py_str_repr(target_name)
        )));
    };
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(target_name)
        )));
    }
    if target
        .weles
        .as_ref()
        .is_some_and(|policy| policy.actions.iter().any(|action| action == "*"))
    {
        return Err(DeployError(format!(
            "target {} carries forbidden wildcard recovery state",
            py_str_repr(target_name)
        )));
    }
    if target.ssh.as_deref().unwrap_or("").is_empty() {
        return Err(DeployError(format!(
            "target {} has no registry-managed ssh destination",
            py_str_repr(target_name)
        )));
    }
    Ok(target)
}

/// [`recover_host`] against an already-loaded registry (Python's
/// `lookup(target_name, source="gcs")` is the caller's concern here).
pub async fn recover_host_with_registry(
    registry: &Registry,
    target_name: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let target = resolve_target(registry, target_name)?;
    let output = runner(CommandSpec {
        argv: ssh_argv(target.ssh.as_deref().unwrap_or("")),
        stdin: Some(remote_script(target)),
        timeout: Some(Duration::from_secs(TIMEOUT_SECONDS)),
    })
    .await
    .map_err(DeployError)?;
    let mut report = parse_output(&output.stdout, target)?;
    report["exit_code"] = json!(output.code);
    if output.code != 0 {
        let detail = output.detail().trim();
        let error = match detail.lines().next_back() {
            Some(last) => last.chars().take(300).collect::<String>(),
            None => "remote recovery failed".to_string(),
        };
        report["error"] = json!(error);
    }
    Ok(report)
}

/// Python `recover_host`: run the fixed recovery procedure on one
/// canonical registry host (the remote registry only — the fleet-survival
/// authority, same as the Python `source="gcs"` lookup; an unreachable
/// store is an error, never an empty registry).
pub async fn recover_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    recover_host_with_registry(&registry, target_name, runner).await
}

/// `json.dumps(report, indent=2, sort_keys=True)` as the CLI prints it.
pub fn to_sorted_pretty(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&map[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string_pretty(&sorted(value)).expect("report serializes")
}

