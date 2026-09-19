//! The attached half of `jeden-session`: place the session, take the kind's
//! hold on the host, then hand this process's streams to the managed Jeden.
//!
//! An attachment lives exactly as long as the caller's streams. That is the
//! contract of `stado workload attach`, and it is why `stado workload start`
//! exists beside it for work that must outlive the terminal it was asked
//! from.

use std::process::Stdio;

use serde_json::json;

use super::{candidates, expand_home, ledger_root, probe_ready, validate_component, MANAGED_JEDEN};
use crate::cli::CmdError;
use crate::deploy::{host_access::ssh_key, host_channel};
use crate::targets::ComputeTarget;

const PLACEMENT_PREFIX: &str = "STADO_JEDEN_PLACEMENT ";

pub(crate) async fn connect_jeden(
    declaration: &crate::cli::workload::WorkloadKind,
    workspace: &str,
    requested_target: Option<&str>,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    validate_component("workspace", workspace)?;
    if let Some(session) = resume {
        validate_component("resume session", session)?;
    }
    let mut hosts = candidates(requested_target).await?;
    if hosts.is_empty() {
        return Err(CmdError::click(
            "the registry declares no reachable local host for jeden-session; add it to the canonical registry",
        ));
    }
    let checkout = super::checkout_path(workspace);
    let mut refusals = Vec::new();
    for target in hosts.drain(..) {
        if let Err(refusal) = probe_ready(&target, &checkout, resume).await {
            refusals.push(refusal);
            continue;
        }
        // The host is ready; take the session's declared hold on it before
        // the runtime starts, so the host publishes itself net of this
        // session from its next tick. A refused hold moves on to the next
        // candidate exactly like a failed probe.
        let holder = format!(
            "{} jeden-session {workspace} pid {}",
            crate::fleet_needs::this_requester(),
            std::process::id()
        );
        let held =
            match crate::cli::capacity::reserve_for_workload(declaration, &target, holder).await? {
                Ok(held) => held,
                Err(refusal) => {
                    refusals.push(refusal.sentence);
                    continue;
                }
            };
        let outcome = attach_jeden(target, workspace, &checkout, resume).await;
        if let Err(error) = held.release().await {
            eprintln!("the session's reservation could not be released: {error}");
        }
        return outcome;
    }
    Err(CmdError::click(format!(
        "no Stado host can run jeden-session in {workspace}; {}",
        refusals.join("; ")
    )))
}

async fn attach_jeden(
    target: ComputeTarget,
    workspace: &str,
    checkout: &str,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    let (_, ledger) = ledger_root(&target);
    eprintln!(
        "{PLACEMENT_PREFIX}{}",
        serde_json::to_string(&json!({
            "kind": "jeden-session",
            "target": target.name,
            "workspace": workspace,
            "cwd": format!("~/{checkout}"),
            "ledger": ledger,
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
