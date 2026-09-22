//! The `jeden-session` workload: place one Jeden session on the host with the
//! most room for it, either attached to this process's streams or detached as
//! a durable queue job that outlives the caller.
//!
//! Placement, the readiness probe and the paths both modes use live here;
//! `attach` hands the streams over, `detached` submits the work to the queue,
//! and `sessions` reads back what the fleet is running.

mod attach;
mod capacity;
mod detached;
mod sessions;

use capacity::{admission_refusal, live_capacity, target_score};

pub(crate) use attach::connect_jeden;
pub(crate) use detached::{start_detached, DetachedRequest};
pub(crate) use sessions::list_sessions;

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

/// The address `service` answers on *from* `target`, as the fleet's own
/// service directory records it.
///
/// A session is placed on whichever host has room, and a loopback service
/// has a different address on each of them, so the address has to be read
/// for the host that will run the session rather than inherited from the
/// machine that asked for it. Without this a session on a host that carries
/// no `~/.jeden/.env` had no model router at all.
pub(super) async fn service_endpoint(service: &str, target: &str) -> Option<String> {
    let (registry, _) = crate::targets::fetch_registry_or_last_good().await.ok()?;
    let endpoint = registry.service(service)?.address_for(target)?;
    Some(endpoint.url.clone())
}

pub(super) const CHECKOUT_ROOT: &str = "Documents/CodingProjects/Wisent";
pub(super) const HOME_WORKSPACE: &str = "__home__";
pub(super) const MANAGED_JEDEN: &str = ".stado/bin/jeden";
pub(super) const MANAGED_STADO: &str = ".stado/bin/stado";
pub(super) const DEFAULT_LEDGER: &str = ".jeden/sessions";
pub(super) const SESSION_ROOT_VARIABLE: &str = "JEDEN_SESSION_ROOT";
/// The grant file a host reads its own Skarbiec credentials through. Jeden
/// asks `stado secrets get` for its signing secret and gateway bearer, and
/// that read needs this file; a host without it gets the same credentials
/// from the fleet, as declared job secrets its agent resolves.
pub(super) const OPERATOR_GRANT_FILE: &str = ".stado/local-operator-skarbiec-token";

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

/// What a candidate host answered about carrying a session.
pub(super) struct HostReadiness {
    /// True when the host holds the operator grant file Jeden reads its own
    /// credentials through. A host without it needs the fleet to hand the
    /// session its credentials as declared job secrets instead.
    pub reads_own_credentials: bool,
}

/// Whether `target` can carry a session in `checkout` right now: the
/// workspace exists, the managed binaries are installed, and the named
/// session ledger is there when one was named. `Err(sentence)` is the
/// host's own reason, kept for the refusal that names every candidate.
pub(super) async fn probe_ready(
    target: &ComputeTarget,
    checkout: &str,
    resume: Option<&str>,
) -> Result<HostReadiness, String> {
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
if [ -r "$HOME"/{OPERATOR_GRANT_FILE} ]; then
  printf 'ready own-credentials'
else
  printf 'ready fleet-credentials'
fi
"#,
    );
    let runner = crate::deploy::production_runner();
    match host_channel::run_script(target, &probe, &runner).await {
        Ok(output) if output.ok() && output.stdout.trim().starts_with("ready") => {
            Ok(HostReadiness {
                reads_own_credentials: output.stdout.trim().ends_with("own-credentials"),
            })
        }
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
