//! The `jeden-session` workload: place one interactive Jeden session on the
//! host with the most room for it, then hand this process's streams over.

use std::process::Stdio;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_access::ssh_key, host_channel};
use crate::targets::ComputeTarget;

pub(crate) fn current_workspace() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| validate_component("workspace", name).is_ok())
        .unwrap_or_else(|| "__home__".to_string())
}

const CHECKOUT_ROOT: &str = "Documents/CodingProjects/Wisent";
const HOME_WORKSPACE: &str = "__home__";
const MANAGED_JEDEN: &str = ".stado/bin/jeden";
const MANAGED_STADO: &str = ".stado/bin/stado";
const PLACEMENT_PREFIX: &str = "STADO_JEDEN_PLACEMENT ";

pub(crate) async fn connect_jeden(
    workspace: &str,
    requested_target: Option<&str>,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    validate_component("workspace", workspace)?;
    if let Some(session) = resume {
        validate_component("resume session", session)?;
    }
    let (registry, canonical) = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, notice)) => {
            if let Some(notice) = notice {
                crate::targets::report_registry_notice(&notice);
            }
            let canonical = registry.staleness_seconds.is_none();
            (registry, canonical)
        }
        Err(_) => (
            crate::targets::load_bundled_registry()
                .map_err(|error| CmdError::click(error.to_string()))?,
            false,
        ),
    };
    let mut candidates = if let Some(name) = requested_target {
        let target = host_channel::resolve_target(&registry, name)
            .map_err(|error| CmdError::click(error.to_string()))?
            .clone();
        if !canonical && !host_channel::target_is_this_host(&target) {
            return Err(CmdError::click(
                "canonical registry is unavailable; refresh it before a remote Jeden reconnect",
            ));
        }
        vec![target]
    } else {
        let capacity = live_capacity().await;
        let mut targets = registry
            .targets
            .iter()
            .filter(|target| {
                target.is_provider(crate::capabilities::ProviderId::Local)
                    && (host_channel::target_is_this_host(target)
                        || (canonical && target.has_ssh_connection()))
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
            "the registry declares no reachable local host for jeden-session; add it to the canonical registry",
        ));
    }
    let runner = crate::deploy::production_runner();
    let mut refusals = Vec::new();
    for target in candidates.drain(..) {
        let checkout = checkout_path(workspace);
        let resume_probe = resume.map(|session| format!(
            "if [ ! -d \"$HOME\"/.jeden/sessions/{session} ]; then printf 'session ledger is missing: %s\\n' \"$HOME\"/.jeden/sessions/{session} >&2; ready=no; fi\n"
        )).unwrap_or_default();
        let probe = format!(
            r#"ready=yes
if [ ! -d "$HOME"/{checkout} ]; then
  printf 'workspace directory is missing: %s\n' "$HOME"/{checkout} >&2
  ready=no
fi
{resume_probe}
for binary in {MANAGED_JEDEN} {MANAGED_STADO}; do
  if [ ! -x "$HOME/$binary" ]; then
    printf 'managed runtime is missing or not executable: %s\n' "$HOME/$binary" >&2
    ready=no
  fi
done
[ "$ready" = yes ] || exit 1
printf ready
"#,
        );
        match host_channel::run_script(&target, &probe, &runner).await {
            Ok(output) if output.ok() && output.stdout.trim() == "ready" => {
                return attach_jeden(target, workspace, &checkout, resume).await;
            }
            Ok(output) => {
                let detail = output.detail();
                refusals.push(format!(
                    "{}: {}",
                    target.name,
                    if detail.trim().is_empty() {
                        "the runtime preflight returned no readiness or failure detail"
                    } else {
                        detail.trim()
                    }
                ));
            }
            Err(error) => refusals.push(format!("{}: {error}", target.name)),
        }
    }
    Err(CmdError::click(format!(
        "no Stado host can run jeden-session in {workspace}; {}",
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
    if safe {
        Ok(())
    } else {
        Err(CmdError::usage(format!(
            "{label} must contain only letters, numbers, dot, dash, or underscore"
        )))
    }
}

fn checkout_path(workspace: &str) -> String {
    if workspace == HOME_WORKSPACE {
        String::new()
    } else {
        format!("{CHECKOUT_ROOT}/{workspace}")
    }
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
    let live = capacity
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("local"))
        .find(|entry| {
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
        });
    let accepting = live
        .and_then(|entry| entry.get("accepting_jobs"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let available_cpu_cores = live
        .and_then(|entry| entry.get("available_cpu_cores"))
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .max(0);
    let live_bonus = if accepting { 1_000_000 } else { 0 };
    let local_bonus = i64::from(host_channel::target_is_this_host(target));
    live_bonus + available_cpu_cores.saturating_mul(1_000) + local_bonus
}

async fn attach_jeden(
    target: ComputeTarget,
    workspace: &str,
    checkout: &str,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    eprintln!(
        "{PLACEMENT_PREFIX}{}",
        serde_json::to_string(&json!({
            "kind": "jeden-session",
            "target": target.name,
            "workspace": workspace,
            "cwd": format!("~/{checkout}"),
            "ledger": "~/.jeden/sessions",
            "resume": resume,
        }))?
    );
    let status = if host_channel::target_is_this_host(&target) {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(expand_home(".stado/bin")?).chain(std::env::split_paths(&inherited)),
        )
        .map_err(|error| {
            CmdError::click(format!(
                "cannot construct the managed runtime PATH: {error}"
            ))
        })?;
        tokio::process::Command::new(expand_home(MANAGED_JEDEN)?)
            .arg("rpc")
            .env("PATH", path)
            .current_dir(expand_home(checkout)?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?
    } else {
        let connection =
            host_channel::select_ssh_connection(&target, &crate::deploy::production_runner())
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        let key = ssh_key::materialize(target.channel_key())
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let mut argv = host_channel::ssh_options(connection.destination);
        argv.insert(1, "-T".to_string());
        argv.push(format!(
            "cd \"$HOME\"/{checkout} && PATH=\"$HOME/.stado/bin:$PATH\" exec \"$HOME\"/{MANAGED_JEDEN} rpc"
        ));
        let argv = ssh_key::add_identity(argv, &key)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| CmdError::click("registry SSH channel is empty; repair the target"))?;
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
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CmdError::click("HOME is not set; set it before attaching"))?;
    Ok(std::path::PathBuf::from(home).join(path))
}
