//! Stado-placed interactive Jeden sessions for native clients.
//!
//! This is deliberately narrower than `host exec`: the remote program is
//! always `jeden rpc`, the working directory is one validated repository name
//! under the Wisent checkout root, and stdin/stdout remain attached so the
//! canonical Jeden RPC stream reaches the desktop without a second protocol.

use std::process::Stdio;

use serde_json::{json, Value};

use super::CmdError;
use crate::deploy::{host_channel, ssh_key};
use crate::targets::ComputeTarget;

const CHECKOUT_ROOT: &str = "Documents/CodingProjects/Wisent";
const PLACEMENT_PREFIX: &str = "STADO_JEDEN_PLACEMENT ";

/// Select a live registry host that owns WORKSPACE, then attach this process's
/// stdio to `jeden rpc` there. `--target` reconnects to the host that owns a
/// durable session; `--resume` makes initial placement require that ledger.
pub async fn connect_jeden(
    workspace: &str,
    requested_target: Option<&str>,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    validate_component("workspace", workspace)?;
    if let Some(session) = resume {
        validate_component("resume session", session)?;
    }
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut candidates = if let Some(name) = requested_target {
        vec![host_channel::resolve_target(&registry, name)
            .map_err(|error| CmdError::click(error.to_string()))?
            .clone()]
    } else {
        let capacity = live_capacity().await;
        let mut targets = registry
            .targets
            .iter()
            .filter(|target| {
                target.is_provider(crate::capabilities::ProviderId::Local)
                    && (host_channel::target_is_this_host(target)
                        || target.ssh.as_deref().is_some_and(|value| !value.is_empty()))
            })
            .cloned()
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| {
            target_score(right, &capacity)
                .cmp(&target_score(left, &capacity))
                .then_with(|| left.name.cmp(&right.name))
        });
        targets
    };

    if candidates.is_empty() {
        return Err(CmdError::click(
            "the canonical registry has no reachable local host for interactive Jeden work",
        ));
    }

    let runner = crate::deploy::production_runner();
    let mut refusals = Vec::new();
    for target in candidates.drain(..) {
        let checkout = checkout_path(workspace);
        let resume_probe = resume
            .map(|session| format!("test -d \"$HOME\"/.jeden/sessions/{session}\n"))
            .unwrap_or_default();
        let probe = format!(
            "set -e\ntest -d \"$HOME\"/{checkout}\n{resume_probe}command -v jeden >/dev/null\nprintf ready\n",
        );
        match host_channel::run_script(&target, &probe, &runner).await {
            Ok(output) if output.ok() && output.stdout.trim() == "ready" => {
                return attach(target, workspace, &checkout).await;
            }
            Ok(output) => refusals.push(format!(
                "{}: {}",
                target.name,
                host_channel::last_error_line(&output, "workspace or jeden executable unavailable")
            )),
            Err(error) => refusals.push(format!("{}: {error}", target.name)),
        }
    }

    Err(CmdError::click(format!(
        "no Stado host can run Jeden in {workspace}: {}",
        refusals.join("; ")
    )))
}

fn validate_component(label: &str, value: &str) -> Result<(), CmdError> {
    let bytes = value.as_bytes();
    let safe = !bytes.is_empty()
        && value != "."
        && value != ".."
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !safe {
        return Err(CmdError::usage(format!(
            "{label} must contain only letters, numbers, dot, dash, or underscore"
        )));
    }
    Ok(())
}

fn checkout_path(workspace: &str) -> String {
    format!("{CHECKOUT_ROOT}/{workspace}")
}

async fn live_capacity() -> Vec<Value> {
    let Ok(store) = crate::queue::submit::default_store("").await else {
        return Vec::new();
    };
    crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map(|entries| entries.into_values().collect())
        .unwrap_or_default()
}

fn target_score(target: &ComputeTarget, capacity: &[Value]) -> i64 {
    let hostnames = target
        .hostnames
        .iter()
        .map(|host| crate::targets::normalize_hostname(host))
        .collect::<Vec<_>>();
    let live_slots = capacity
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("local"))
        .filter(|entry| {
            let consumer = entry
                .get("consumer_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            hostnames.iter().any(|host| {
                consumer == format!("local-{host}")
                    || consumer
                        .strip_prefix("local-")
                        .is_some_and(|value| crate::targets::normalize_hostname(value) == *host)
            })
        })
        .flat_map(|entry| {
            entry
                .get("free_slots")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|slots| slots.values())
        })
        .filter_map(Value::as_i64)
        .sum::<i64>();
    let live_bonus = if live_slots > 0 { 1_000_000 } else { 0 };
    let local_bonus = if host_channel::target_is_this_host(target) {
        1
    } else {
        0
    };
    live_bonus + live_slots.saturating_mul(1_000) + target.slots.max(0) + local_bonus
}

async fn attach(target: ComputeTarget, workspace: &str, checkout: &str) -> Result<(), CmdError> {
    eprintln!(
        "{PLACEMENT_PREFIX}{}",
        serde_json::to_string(&json!({
            "target": target.name,
            "workspace": workspace,
            "cwd": format!("~/{checkout}"),
        }))?
    );

    let status = if host_channel::target_is_this_host(&target) {
        tokio::process::Command::new("jeden")
            .arg("rpc")
            .current_dir(expand_home(checkout)?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?
    } else {
        let key = ssh_key::materialize(&target.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let mut argv = host_channel::ssh_options(target.ssh.as_deref().unwrap_or_default());
        argv.insert(1, "-T".to_string());
        argv.push(format!("cd \"$HOME\"/{checkout}; exec jeden rpc"));
        let argv = ssh_key::add_identity(argv, &key)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| CmdError::click("registry SSH channel is empty"))?;
        let result = tokio::process::Command::new(program)
            .args(arguments)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?;
        drop(key);
        result
    };

    if status.success() {
        Ok(())
    } else {
        Err(CmdError::silent(status.code().unwrap_or(1)))
    }
}

fn expand_home(path: &str) -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CmdError::click("HOME is not set"))?;
    Ok(std::path::PathBuf::from(home).join(path))
}
