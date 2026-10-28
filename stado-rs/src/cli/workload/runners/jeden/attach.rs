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
        )
        .stating(crate::primitives::failure::FailureCode::Config));
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
            crate::fleet_needs::this_requester()?,
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
        let outcome = attach_jeden(
            target,
            workspace,
            &checkout,
            resume,
            declaration.park_after_seconds,
        )
        .await;
        if let Err(error) = held.release().await {
            eprintln!("the session's reservation could not be released: {error}");
        }
        return outcome;
    }
    Err(CmdError::refused(format!(
        "no Stado host can run jeden-session in {workspace}; {}",
        refusals.join("; ")
    )))
}

/// The setting `jeden rpc` reads its park time from.
const PARK_AFTER_SETTING: &str = "JEDEN_PARK_AFTER_SECONDS";

async fn attach_jeden(
    target: ComputeTarget,
    workspace: &str,
    checkout: &str,
    resume: Option<&str>,
    park_after_seconds: Option<u64>,
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
            "park_after_seconds": park_after_seconds,
        }))?
    );
    let status = if host_channel::target_is_this_host(&target) {
        // The managed runtime first, then whatever PATH this process was given;
        // a process started with no PATH contributes no further directories.
        let inherited: Vec<std::path::PathBuf> = std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .collect();
        let path =
            std::env::join_paths(std::iter::once(expand_home(".stado/bin")?).chain(inherited))
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot construct the managed runtime PATH: {error}"
                    ))
                    .stating(crate::primitives::failure::FailureCode::Config)
                })?;
        let mut command = tokio::process::Command::new(expand_home(MANAGED_JEDEN)?);
        command.arg("rpc").env("PATH", path);
        if let Some(seconds) = park_after_seconds {
            command.env(PARK_AFTER_SETTING, seconds.to_string());
        }
        command
            .current_dir(expand_home(checkout)?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        crate::wait::status_async(&mut command).await?
    } else {
        let connection =
            host_channel::select_ssh_connection(&target, &crate::deploy::production_runner())
                .await
                .map_err(CmdError::from)?;
        let key = ssh_key::materialize(target.channel_key())
            .await
            .map_err(CmdError::from)?;
        let mut argv = host_channel::ssh_options(connection.destination);
        // No terminal on the far side: among the options, before the
        // destination that ends the argv.
        if let Some(destination) = argv.pop() {
            argv.push("-T".to_string());
            argv.push(destination);
        }
        let park = match park_after_seconds {
            Some(seconds) => format!("{PARK_AFTER_SETTING}={seconds} "),
            None => String::new(),
        };
        argv.push(format!(
            "cd \"$HOME\"/{checkout} && PATH=\"$HOME/.stado/bin:$PATH\" {park}exec \"$HOME\"/{MANAGED_JEDEN} rpc"
        ));
        let argv = ssh_key::add_identity(argv, &key).map_err(CmdError::from)?;
        let (program, arguments) = argv.split_first().ok_or_else(|| {
            CmdError::declaration("registry SSH channel is empty; repair the target")
        })?;
        let result = crate::wait::status_async(
            tokio::process::Command::new(program)
                .args(arguments)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .kill_on_drop(true),
        )
        .await?;
        drop(key);
        result
    };
    match status.code() {
        _ if status.success() => Ok(()),
        Some(code) => Err(CmdError::silent(code)),
        None => Err(CmdError::click(format!(
            "the managed jeden rpc session ended with {status} and returned no exit code"
        ))),
    }
}
