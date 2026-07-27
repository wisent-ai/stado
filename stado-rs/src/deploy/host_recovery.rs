//! Registry-authorized recovery for a managed macOS host.
//!
//! Port of `stado/deploy/host_recovery.py`. The remote program is fixed
//! and deliberately narrow: run the canonical disk cleanup, disable the
//! obsolete local coordinator, and reload known LaunchAgents. Registry
//! data selects only the host; it cannot supply shell fragments.
//!
//! The tab-delimited `STADO_*` marker protocol (script emission in
//! [`remote_script`], parsing in [`parse_output`]) is byte-exact with the
//! Python — including the deliberate mix of literal `\t` / `\n` escape
//! sequences and real control characters in the printf format strings.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{py_str_repr, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::{normalize_hostname, ssh_hostname, ComputeTarget, Registry};

/// Python `_TIMEOUT_SECONDS`.
pub const TIMEOUT_SECONDS: u64 = 120;

/// Python `_WC_CANDIDATES`.
pub const WC_CANDIDATES: [&str; 3] = [
    "$HOME/.venvs/wisent-compute/bin/wc",
    "$HOME/.local/bin/wc",
    "/opt/homebrew/bin/wc",
];

/// Python `_MANAGED_AGENTS` (label, plist path) pairs, in order.
pub const MANAGED_AGENTS: [(&str, &str); 5] = [
    (
        "com.wisent.compute.auto-deployer",
        "$HOME/Library/LaunchAgents/com.wisent.compute.auto-deployer.plist",
    ),
    (
        "com.wisent.weles-auto-deploy",
        "$HOME/Library/LaunchAgents/com.wisent.weles-auto-deploy.plist",
    ),
    (
        "com.wisent.weles-worker",
        "$HOME/Library/LaunchAgents/com.wisent.weles-worker.plist",
    ),
    (
        "com.wisent.weles-keyword-planner-api",
        "$HOME/Library/LaunchAgents/com.wisent.weles-keyword-planner-api.plist",
    ),
    (
        "com.wisent.host-health-beacon",
        "$HOME/Library/LaunchAgents/com.wisent.host-health-beacon.plist",
    ),
];

/// The fixed remote program with `@IDENTITY_WORDS@` / `@WC_WORDS@` /
/// `@AGENT_ROWS@` substitution points. Written with explicit escapes so it
/// is byte-exact with the Python f-string render: `\t` / `\r` / `\n` are
/// real control characters, `\\t` / `\\n` are literal backslash sequences
/// (the Python source mixes both on purpose).
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
    if target.kind != "local" {
        return Err(DeployError(format!(
            "target {} is not a local host",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{runner_fn, CommandOutput};

    fn target() -> ComputeTarget {
        ComputeTarget {
            name: "mini-one".to_string(),
            kind: "local".to_string(),
            gpu_type: None,
            slots: 1,
            ssh: Some("wisent@mini-one.local".to_string()),
            region: None,
            spot: false,
            max_concurrent: None,
            team_id: None,
            notes: String::new(),
            hostnames: vec!["mini-one.lan".to_string(), "mini-one.local".to_string()],
            weles: None,
            disk_cleanup: None,
            env_overrides: Default::default(),
            agent_args: Vec::new(),
            vram_gb: None,
            pinned_only: false,
            extra: Default::default(),
        }
    }

    #[test]
    fn identity_values_are_normalized_and_sorted() {
        assert_eq!(
            identity_values(&target()),
            vec!["mini-one", "mini-one.lan", "mini-one.local"]
        );
    }

    #[test]
    fn remote_script_matches_python_golden() {
        assert_eq!(
            remote_script(&target()),
            include_str!("testdata/host_recovery_remote_script.sh")
        );
    }

    #[test]
    fn ssh_argv_matches_python() {
        assert_eq!(
            ssh_argv("wisent@mini-one.local"),
            vec![
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=15",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "wisent@mini-one.local",
                "/bin/bash",
                "-s",
            ]
        );
    }

    #[test]
    fn parse_ok_report_with_all_markers() {
        let stdout = "STADO_DOMAIN\tgui/501\tavailable\n\
                      STADO_AGENT\tcom.wisent.weles-worker\trestarted\n\
                      STADO_AGENT\tcom.wisent.host-health-beacon\tmissing_plist\n\
                      STADO_RECOVER\tok\tmini-one\t1024\t2048\tok\n\
                      STADO_CLEANUP\t{\"outcome\": \"cleaned\", \"freed_bytes\": 10}\n";
        let report = parse_output(stdout, &target()).unwrap();
        assert_eq!(report["status"], "ok");
        assert_eq!(report["host"], "mini-one");
        assert_eq!(report["disk_free_kb_before"], 1024);
        assert_eq!(report["disk_free_kb_after"], 2048);
        assert_eq!(report["cleanup_status"], "ok");
        assert_eq!(
            report["launchd_domain"],
            json!({"name": "gui/501", "status": "available"})
        );
        assert_eq!(
            report["agents"],
            json!({
                "com.wisent.weles-worker": "restarted",
                "com.wisent.host-health-beacon": "missing_plist",
            })
        );
        assert_eq!(
            report["cleanup"],
            json!({"outcome": "cleaned", "freed_bytes": 10})
        );
        assert!(report.get("remote_error").is_none());
    }

    #[test]
    fn parse_identity_mismatch_and_invalid_cleanup_json() {
        let stdout = "STADO_RECOVER\tidentity_mismatch\tother-host\n";
        let report = parse_output(stdout, &target()).unwrap();
        assert_eq!(report["status"], "failed");
        assert_eq!(
            report["remote_error"],
            json!(["identity_mismatch", "other-host"])
        );

        let stdout = "STADO_CLEANUP\tnot json\n";
        let report = parse_output(stdout, &target()).unwrap();
        assert_eq!(report["cleanup"], json!({"outcome": "invalid_output"}));
    }

    #[test]
    fn parse_rejects_non_integer_disk_fields() {
        let stdout = "STADO_RECOVER\tok\tmini-one\tmany\t2048\tok\n";
        let err = parse_output(stdout, &target()).unwrap_err();
        assert_eq!(err.0, "invalid literal for int() with base 10: 'many'");
    }

    #[test]
    fn resolve_target_errors_match_python() {
        let mut registry = Registry::default();
        let err = super::resolve_target(&registry, "ghost").unwrap_err();
        assert_eq!(err.0, "target 'ghost' is not in the canonical registry");

        let mut gcp = target();
        gcp.name = "gcp-box".to_string();
        gcp.kind = "gcp".to_string();
        registry.targets.push(gcp);
        let err = super::resolve_target(&registry, "gcp-box").unwrap_err();
        assert_eq!(err.0, "target 'gcp-box' is not a local host");

        let mut nossh = target();
        nossh.name = "no-ssh".to_string();
        nossh.ssh = None;
        registry.targets.push(nossh);
        let err = super::resolve_target(&registry, "no-ssh").unwrap_err();
        assert_eq!(
            err.0,
            "target 'no-ssh' has no registry-managed ssh destination"
        );
    }

    #[tokio::test]
    async fn recover_host_parses_markers_and_records_exit_code() {
        let mut registry = Registry::default();
        registry.targets.push(target());
        let runner = runner_fn(|spec| async move {
            assert!(spec.argv.starts_with(&["ssh".to_string()]));
            assert!(spec.stdin.as_deref().unwrap().contains("STADO_RECOVER"));
            Ok(CommandOutput {
                code: 0,
                stdout: "STADO_DOMAIN\tgui/501\tfallback\nSTADO_RECOVER\tok\tmini-one\t5\t9\tunavailable\n".to_string(),
                stderr: String::new(),
            })
        });
        let report = recover_host_with_registry(&registry, "mini-one", &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], "ok");
        assert_eq!(report["exit_code"], 0);
        assert_eq!(report["launchd_domain"]["status"], "fallback");
        assert!(report.get("error").is_none());
    }

    #[tokio::test]
    async fn recover_host_nonzero_exit_captures_last_stderr_line() {
        let mut registry = Registry::default();
        registry.targets.push(target());
        let runner = runner_fn(|_spec| async move {
            Ok(CommandOutput {
                code: 64,
                stdout: String::new(),
                stderr: "line one\nidentity mismatch detail\n".to_string(),
            })
        });
        let report = recover_host_with_registry(&registry, "mini-one", &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], "failed");
        assert_eq!(report["exit_code"], 64);
        assert_eq!(report["error"], "identity mismatch detail");

        let runner = runner_fn(|_spec| async move {
            Ok(CommandOutput {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
            })
        });
        let report = recover_host_with_registry(&registry, "mini-one", &runner)
            .await
            .unwrap();
        assert_eq!(report["error"], "remote recovery failed");
    }

    #[test]
    fn sorted_pretty_orders_keys() {
        let report = json!({"status": "ok", "agents": {"b": 1, "a": 2}, "target": "t"});
        let pretty = to_sorted_pretty(&report);
        assert_eq!(
            pretty,
            "{\n  \"agents\": {\n    \"a\": 2,\n    \"b\": 1\n  },\n  \"status\": \"ok\",\n  \"target\": \"t\"\n}"
        );
    }
}
