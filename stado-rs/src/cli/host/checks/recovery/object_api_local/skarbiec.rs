//! The Skarbiec bootstrap readers of the object-API recovery. An interrupted
//! handoff may leave Stado's exact Skarbiec proxy alive with no recorded
//! release owner and a dead candidate upstream; the recovery breaks that
//! cycle only from coordinates the host's registry supplies, and these
//! readers never guess a bind, state path, plist, label or readiness contract.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::Value;

use super::config::absolute;
use super::config::home as home_dir;

/// The largest TCP port.
const PORT_MAX: u64 = u16::MAX as u64;
/// The declared blue-green strategy keeps exactly two candidate ports.
const CANDIDATE_PORTS: usize = 2;
/// The longest readiness wait, in seconds, a registry may declare for the
/// Skarbiec strategy; a larger declaration is refused as invalid.
const DECLARED_READINESS_WAIT_MAX: u64 = 600;
/// The registry key that declares that wait (`release_control.products.
/// skarbiec.strategy`).
const READINESS_WAIT_KEY: &str = "readiness_timeout_seconds";

fn refuse(detail: &str) -> String {
    format!("skarbiec bootstrap refused: {detail}")
}

fn read(path: &Path) -> Result<Value, String> {
    std::fs::read(path)
        .map_err(|error| format!("{}: {error}", path.display()))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display())))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(refuse(&format!("{key} must be a string"))),
    }
}

/// `managed\tTARGET\tRELEASE_STATE\tPROXY_STATE\tBIND\tPORTS\tREADINESS\tLEGACY_PLIST\tLEGACY_LABEL\tWAIT`,
/// or `absent` when the registry does not manage Skarbiec.
pub(super) fn plan(registry: &Path, host: &str, account: &str) -> Result<String, String> {
    let document = read(registry)?;
    let Some(policy) = document.pointer("/release_control/products/skarbiec").filter(|v| v.is_object()) else {
        return Ok("absent".into());
    };
    let strategy = policy.get("strategy").cloned().unwrap_or(Value::Null);
    if strategy.get("kind").and_then(Value::as_str) != Some("blue-green") {
        return Err(refuse("release strategy is not blue-green"));
    }
    let targets = policy.get("targets").and_then(Value::as_object).cloned().unwrap_or_default();
    let home = home_dir();
    let target_name = if targets.contains_key(host) {
        host.to_string()
    } else {
        let matches: Vec<&String> = targets
            .iter()
            .filter(|(_, target)| {
                target.get("run_as_user").and_then(Value::as_str) == Some(account)
                    && absolute(target.get("home").and_then(Value::as_str).unwrap_or(""), &home) == home
            })
            .map(|(name, _)| name)
            .collect();
        let [only] = matches.as_slice() else {
            return Err(refuse("registry does not identify this host exactly"));
        };
        (*only).clone()
    };
    let target = &targets[&target_name];
    let state_dir = string(target, "state_dir")?;
    let stable_bind = string(target, "stable_bind")?;
    let readiness_path = string(target, "readiness_path")?;
    if [state_dir, stable_bind, readiness_path].iter().any(|value| value.is_empty()) {
        return Err(refuse("release target is incomplete"));
    }
    let (legacy_plist, legacy_label) = match (string(target, "legacy_launchd_plist"), string(target, "legacy_launchd_label")) {
        (Ok(plist), Ok(label)) => (plist, label),
        _ => return Err(refuse("legacy plist and label must be strings")),
    };
    let legacy = !legacy_plist.is_empty() && !legacy_label.is_empty();
    if legacy != (!legacy_plist.is_empty() || !legacy_label.is_empty()) {
        return Err(refuse("legacy plist and label must be declared together"));
    }
    let mut checked = vec![state_dir, stable_bind, readiness_path];
    if legacy {
        checked.extend([legacy_plist, legacy_label]);
    }
    if checked.iter().any(|value| value.contains(['\t', '\r', '\n'])) {
        return Err(refuse("release target contains control characters"));
    }
    let port = match stable_bind.split_once(':') {
        Some(("127.0.0.1", port)) => port.parse::<u64>().unwrap_or(0),
        _ => return Err(refuse("stable bind is not loopback")),
    };
    if !(1..=PORT_MAX).contains(&port) {
        return Err(refuse("stable bind port is invalid"));
    }
    if !readiness_path.starts_with('/') || readiness_path.contains(char::is_whitespace) {
        return Err(refuse("readiness path is invalid"));
    }
    let label_ok = legacy_label.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c));
    if legacy && (!label_ok || legacy_plist != format!("/Library/LaunchDaemons/{legacy_label}.plist")) {
        return Err(refuse("legacy launchd identity is invalid"));
    }
    let declared_ports = target.get("candidate_ports").and_then(Value::as_array).cloned().unwrap_or_default();
    let ports: Vec<u64> = declared_ports
        .iter()
        .filter_map(Value::as_u64)
        .filter(|port| (1..=PORT_MAX).contains(port))
        .collect();
    if ports.len() != CANDIDATE_PORTS || declared_ports.len() != ports.len() {
        return Err(refuse("candidate ports are invalid"));
    }
    let wait = strategy.get(READINESS_WAIT_KEY).and_then(Value::as_u64).unwrap_or(0);
    if !(1..=DECLARED_READINESS_WAIT_MAX).contains(&wait) {
        return Err(refuse("readiness wait is invalid"));
    }
    let state_dir = absolute(state_dir, &home);
    let (legacy_plist, legacy_label) = if legacy {
        (absolute(legacy_plist, &home).display().to_string(), legacy_label.to_string())
    } else {
        ("-".into(), "-".into())
    };
    Ok([
        "managed".to_string(),
        target_name,
        state_dir.join("skarbiec.json").display().to_string(),
        state_dir.join("skarbiec-proxy.json").display().to_string(),
        stable_bind.to_string(),
        ports.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
        readiness_path.to_string(),
        legacy_plist,
        legacy_label,
        wait.to_string(),
    ]
    .join("\t"))
}

/// `owned` when the release state records any release or a proxy pid.
pub(super) fn ownership(state: &Path, target: &str) -> Result<String, String> {
    let state = read(state)?;
    if state.get("product").and_then(Value::as_str) != Some("skarbiec")
        || state.get("target").and_then(Value::as_str) != Some(target)
    {
        return Err(refuse("release state identity differs"));
    }
    let recorded = |key: &str| state.get(key).is_some_and(|value| !value.is_null());
    let owned = ["active", "candidate", "previous", "proxy_pid"].iter().any(|key| recorded(key));
    Ok(if owned { "owned" } else { "unowned" }.into())
}

/// The proxy's upstream, refused unless it is one of the declared candidates.
pub(super) fn upstream(state: &Path, ports: &str) -> Result<String, String> {
    let state = read(state)?;
    let upstream = state.get("upstream").and_then(Value::as_str).unwrap_or("");
    if !ports.split(',').any(|port| upstream == format!("127.0.0.1:{port}")) {
        return Err(refuse("proxy upstream is not a declared candidate"));
    }
    Ok(upstream.to_string())
}

/// `none`, or `exact\tPID\tEXECUTABLE` for the one live process running
/// exactly `stado release proxy --state STATE --bind BIND` from an
/// executable file; more than one is refused.
pub(super) fn proxy_match(processes: &Path, state: &str, bind: &str) -> Result<String, String> {
    let listing = std::fs::read(processes).map_err(|error| format!("{}: {error}", processes.display()))?;
    let expected = ["release", "proxy", "--state", state, "--bind", bind];
    let mut matches = Vec::new();
    for line in String::from_utf8_lossy(&listing).lines() {
        let Some((pid, command)) = line.trim().split_once(char::is_whitespace) else { continue };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let argv: Vec<&str> = command.split_whitespace().collect();
        let Some((program, arguments)) = argv.split_first() else { continue };
        let executable = Path::new(program);
        let runnable = std::fs::metadata(executable)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
        if arguments == expected && executable.file_name().is_some_and(|name| name == "stado") && runnable {
            matches.push((pid.to_string(), program.to_string()));
        }
    }
    match matches.as_slice() {
        [] => Ok("none".into()),
        [(pid, executable)] => Ok(format!("exact\t{pid}\t{executable}")),
        many => Err(refuse(&format!("{} exact release proxies found", many.len()))),
    }
}
