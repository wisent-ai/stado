//! Bootstrap stage four: walk the target list, carry each target's scoped
//! SSH key into its provision, and decide between the SSH-based remote
//! install and the local launchd/systemd --user install.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::deploy::local_install::{self, TokenFetcher};
use crate::deploy::{host_access::ssh_key, runner_fn, DeployError, Runner};
use crate::targets::{ComputeTarget, Registry};

use super::provision::provision_target;

/// Python `run`: provision every target, echoing per-target failures as
/// `[err]  {name}: {exc}` and continuing.
pub async fn run(
    targets: &[&ComputeTarget],
    dry_run: bool,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) {
    for target in targets {
        if let Err(exc) = provision_target(target, dry_run, runner, echo).await {
            echo(&format!("[err]  {}: {exc}", target.name));
        }
    }
}

/// Production remote bootstrap: every SSH/SCP operation carries the
/// registry target's scoped private key. The key exists only for this target
/// provision and is deleted when the keyed runner is dropped.
async fn run_with_target_keys(
    targets: &[&ComputeTarget],
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) {
    for target in targets {
        let key = match ssh_key::materialize(target.channel_key()).await {
            Ok(key) => Arc::new(key),
            Err(exc) => {
                echo(&format!("[err]  {}: {exc}", target.name));
                continue;
            }
        };
        let base_runner = Arc::clone(runner);
        let keyed_runner = runner_fn(move |mut spec| {
            let base_runner = Arc::clone(&base_runner);
            let key = Arc::clone(&key);
            async move {
                if matches!(spec.argv.first().map(String::as_str), Some("ssh" | "scp")) {
                    spec.argv = ssh_key::add_identity(spec.argv, &key)
                        .map_err(|error| error.to_string())?;
                }
                base_runner(spec).await
            }
        });
        if let Err(exc) = provision_target(target, false, &keyed_runner, echo).await {
            echo(&format!("[err]  {}: {exc}", target.name));
        }
    }
}

/// Python `run_bootstrap`: top-level dispatcher used by `stado bootstrap`.
/// Decides between the SSH-based remote install and the local
/// launchd/systemd --user install, and accepts either a kind=local target
/// or a runtime=daemon coordinator.
pub async fn run_bootstrap(
    registry: &Registry,
    target: Option<&str>,
    dry_run: bool,
    local_install_flag: bool,
    runner: &Runner,
    hf_fetch: &TokenFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    if local_install_flag {
        let Some(target) = target else {
            return Err("--local requires --target NAME".into());
        };
        // `--local` installs on THIS machine, so this machine's own
        // declaration decides the domain — for the agent, for a coordinator
        // tick, and for the two internal daemons alike. An always-on host has
        // no per-login domain, and every unit this path wrote into
        // `~/Library/LaunchAgents` there is a unit launchd never loaded.
        let daemon_domain = registry
            .lookup_self(&crate::providers::vast::system_hostname())
            .ok()
            .flatten()
            .is_some_and(crate::deploy::service::requires_daemon_domain);
        // Special target: failure-fixer is a wisent-compute-internal
        // daemon, not a registry coordinator entry. Treated like the
        // local install path but with kind=failure-fixer so the
        // ExecArgs come from the bash-loop branch in
        // local_install.exec_args_for.
        if target == "failure-fixer" {
            return local_install::install_local(
                "failure-fixer",
                "failure-fixer",
                dry_run,
                daemon_domain,
                runner,
                hf_fetch,
                echo,
            )
            .await;
        }
        if target == "watchdog" {
            return local_install::install_local(
                "watchdog",
                "watchdog",
                dry_run,
                daemon_domain,
                runner,
                hf_fetch,
                echo,
            )
            .await;
        }
        if let Some(t) = registry.lookup(target) {
            if t.is_provider(crate::capabilities::ProviderId::Local) {
                return local_install::install_local(
                    &t.name,
                    "agent",
                    dry_run,
                    daemon_domain,
                    runner,
                    hf_fetch,
                    echo,
                )
                .await;
            }
        }
        if let Some(c) = registry.lookup_coordinator(target) {
            if c.runtime == "daemon" || c.runtime == "cron" {
                return local_install::install_local(
                    &c.name,
                    "coordinator",
                    dry_run,
                    daemon_domain,
                    runner,
                    hf_fetch,
                    echo,
                )
                .await;
            }
            if c.runtime == "gcp_cloud_function" {
                return Err(format!(
                    "coordinator '{target}' runtime=gcp_cloud_function: deployed via CI, \
                     not provisionable as a local service."
                )
                .into());
            }
        }
        return Err(format!("'{target}' not found in registry (or wrong kind/runtime)").into());
    }

    let targets: Vec<&ComputeTarget> = match target {
        Some(name) => {
            let Some(t) = registry.lookup(name) else {
                return Err(format!("target '{name}' not found in registry").into());
            };
            vec![t]
        }
        None => registry.local_targets(),
    };
    if dry_run {
        run(&targets, true, runner, echo).await;
    } else {
        run_with_target_keys(&targets, runner, echo).await;
    }
    Ok(())
}

/// A [`TokenFetcher`] that always yields an empty token (dry-run tests and
/// offline callers).
pub fn empty_hf_fetcher() -> TokenFetcher {
    Arc::new(|| Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, Result<String, String>>)
}
