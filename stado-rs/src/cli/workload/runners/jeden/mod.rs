//! The `jeden-session` workload: place one Jeden session on the host with the
//! most room for it, either attached to this process's streams or detached as
//! a durable queue job that outlives the caller.
//!
//! Placement, the readiness probe and the paths both modes use live here;
//! `attach` hands the streams over, `detached` submits the work to the queue,
//! and `sessions` reads back what the fleet is running.

mod attach;
mod detached;
mod sessions;

pub(crate) use attach::connect_jeden;
pub(crate) use detached::{start_detached, DetachedRequest};
pub(crate) use sessions::list_sessions;

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;
use serde_json::Value;

pub(super) const CHECKOUT_ROOT: &str = "Documents/CodingProjects/Wisent";
pub(super) const HOME_WORKSPACE: &str = "__home__";
pub(super) const MANAGED_JEDEN: &str = ".stado/bin/jeden";
pub(super) const MANAGED_STADO: &str = ".stado/bin/stado";
pub(super) const DEFAULT_LEDGER: &str = ".jeden/sessions";
pub(super) const SESSION_ROOT_VARIABLE: &str = "JEDEN_SESSION_ROOT";

pub(crate) fn current_workspace() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| validate_component("workspace", name).is_ok())
        .unwrap_or_else(|| HOME_WORKSPACE.to_string())
}

pub(super) fn validate_component(label: &str, value: &str) -> Result<(), CmdError> {
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

pub(super) fn checkout_path(workspace: &str) -> String {
    if workspace == HOME_WORKSPACE {
        String::new()
    } else {
        format!("{CHECKOUT_ROOT}/{workspace}")
    }
}

/// The shell expression for the workspace directory on the host, so an empty
/// checkout (the home workspace) does not produce a trailing slash nobody
/// asked for.
pub(super) fn workspace_expression(checkout: &str) -> String {
    if checkout.is_empty() {
        "\"$HOME\"".to_string()
    } else {
        format!("\"$HOME\"/{checkout}")
    }
}

/// One shell word, quoted so a task sentence cannot become shell syntax.
pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Where the harness keeps its ledgers on `target`: the shell expression the
/// readiness probe tests, and the path the placement line reports.
///
/// The harness lets its caller move the ledgers with `JEDEN_SESSION_ROOT` and
/// the local `jeden rpc` inherits this process's environment, so a session
/// this host runs is opened from that root and the probe has to look there
/// too; before this the probe tested `~/.jeden/sessions` while the session
/// opened elsewhere, and refused a ledger that was present. Another host
/// never sees this process's environment, so it keeps the default.
pub(super) fn ledger_root(target: &ComputeTarget) -> (String, String) {
    let moved = host_channel::target_is_this_host(target)
        .then(|| std::env::var(SESSION_ROOT_VARIABLE).ok())
        .flatten()
        .filter(|root| !root.is_empty());
    match moved {
        Some(root) => (format!("'{}'", root.replace('\'', "'\\''")), root),
        None => (
            format!("\"$HOME\"/{DEFAULT_LEDGER}"),
            format!("~/{DEFAULT_LEDGER}"),
        ),
    }
}

pub(super) fn expand_home(path: &str) -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CmdError::click("HOME is not set; set it before attaching"))?;
    Ok(std::path::PathBuf::from(home).join(path))
}

/// Every host that may carry a Jeden session, best first.
///
/// With a target named, that one host and nothing else. Without one, every
/// reachable local target ordered by what its agent published: a host that is
/// accepting work outranks one that is not, then free CPU cores, then this
/// machine as the tie-break. That ordering is what spreads sessions over the
/// fleet instead of piling them onto the machine the operator happens to sit
/// in front of.
pub(super) async fn candidates(
    requested_target: Option<&str>,
) -> Result<Vec<ComputeTarget>, CmdError> {
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
    if let Some(name) = requested_target {
        let target = host_channel::resolve_target(&registry, name)
            .map_err(|error| CmdError::click(error.to_string()))?
            .clone();
        if !canonical && !host_channel::target_is_this_host(&target) {
            return Err(CmdError::click(
                "canonical registry is unavailable; refresh it before a remote Jeden reconnect",
            ));
        }
        return Ok(vec![target]);
    }
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
    Ok(targets)
}

/// Whether `target` can carry a session in `checkout` right now: the
/// workspace exists, the managed binaries are installed, and the named
/// session ledger is there when one was named. `Ok(Err(sentence))` is the
/// host's own reason, kept for the refusal that names every candidate.
pub(super) async fn probe_ready(
    target: &ComputeTarget,
    checkout: &str,
    resume: Option<&str>,
) -> Result<(), String> {
    let (ledger, _) = ledger_root(target);
    let workspace = workspace_expression(checkout);
    let resume_probe = resume
        .map(|session| {
            format!(
                "if [ ! -d {ledger}/{session} ]; then printf 'session ledger is missing: %s\\n' {ledger}/{session} >&2; ready=no; fi\n"
            )
        })
        .unwrap_or_default();
    let probe = format!(
        r#"ready=yes
if [ ! -d {workspace} ]; then
  printf 'workspace directory is missing: %s\n' {workspace} >&2
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
    let runner = crate::deploy::production_runner();
    match host_channel::run_script(target, &probe, &runner).await {
        Ok(output) if output.ok() && output.stdout.trim() == "ready" => Ok(()),
        Ok(output) => {
            let detail = output.detail();
            Err(format!(
                "{}: {}",
                target.name,
                if detail.trim().is_empty() {
                    "the runtime preflight returned no readiness or failure detail"
                } else {
                    detail.trim()
                }
            ))
        }
        Err(error) => Err(format!("{}: {error}", target.name)),
    }
}

pub(super) async fn live_capacity() -> Vec<Value> {
    let Ok(store) = crate::queue::submit::default_store("").await else {
        return Vec::new();
    };
    crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map(|entries| entries.into_values().collect())
        .unwrap_or_default()
}

pub(super) fn target_score(target: &ComputeTarget, capacity: &[Value]) -> i64 {
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
