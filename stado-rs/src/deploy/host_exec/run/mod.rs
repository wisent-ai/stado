//! Running one approved command on a canonical registry host, and the exact
//! document that run prints.

mod resolution;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::deploy::host_channel;
use crate::deploy::{DeployError, Runner};

use super::allowlist::{approve, home_rooted, PROBIERZ_RUN_ROOT_CREATE};
use super::channel::{
    account_program, account_script, candidate_script, home_rooted_script, probierz_run_root_script,
};
use super::refusal::ExecRefusal;
use resolution::extract_resolved_executable;

/// The exact document `stado host exec --json` prints.
///
/// Typed, and `deny_unknown_fields`, because this is a machine document: one
/// of our own reconcile scripts reads it with jq, so a misspelled or renamed
/// key has to be a compile error here rather than a quietly different report
/// its consumer discovers in production. `schema` is what a consumer gates a
/// version on, and a map carrying only the target and its connection detail
/// gives it nothing to gate on.
///
/// Built directly instead of by validating a
/// [`host_channel::base_report`] map on the way out: that map's string keys
/// would be a second shape for the same document, checked no earlier than
/// the call that happens to exercise it, whereas one struct cannot drift
/// from itself. Field names, and the omission of the conditional ones,
/// reproduce the map this replaced, so the printed document is unchanged.

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostExecConnection {
    kind: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
}

impl From<host_channel::UsedConnection<'_>> for HostExecConnection {
    fn from(connection: host_channel::UsedConnection<'_>) -> Self {
        match connection {
            host_channel::UsedConnection::Local => Self {
                kind: "local".into(),
                name: "local".into(),
                destination: None,
            },
            host_channel::UsedConnection::Ssh(connection) => Self {
                kind: "ssh".into(),
                name: connection.name.into(),
                destination: Some(connection.destination.into()),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostExecReceipt {
    schema: String,
    target: String,
    ssh: Option<String>,
    ssh_fallbacks: Vec<crate::targets::SshConnectionPath>,
    /// The local channel or exact declared SSH route that carried the command.
    used_connection: HostExecConnection,
    command: String,
    argv: Vec<String>,
    /// Only for an entry installed at more than one path: `argv[0]` is then
    /// one candidate among several and is not evidence of where the host
    /// found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    program_candidates: Option<Vec<String>>,
    /// Only when the host reported which candidate it execed. The account
    /// script resolves `$program` in the remote shell and reports nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_executable: Option<String>,
    /// Only for an account-owned entry, which runs under its own budget.
    /// Without it a channel cut at the cap reads like a program that failed
    /// fast, and the two ask for different next steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u64>,
    stdout: String,
    stderr: String,
    exit_code: i32,
    status: String,
    /// Only on failure: the remote's own last line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `status` for a command that ran and exited clean.
pub const OK_STATUS: &str = "ok";

/// Run one approved command on a canonical registry host.
///
/// The error type is [`ExecRefusal`] rather than [`DeployError`] so the
/// allowlist's own refusal keeps the code it stated all the way to the
/// operator. Every other failure on this path converts in through
/// `From<DeployError>` with no code, which is the honest answer for a
/// sentence produced by ssh, the registry or the remote shell.
pub async fn exec_host(
    target_name: &str,
    words: &[String],
    runner: &Runner,
) -> Result<Value, ExecRefusal> {
    // Refuse before resolving the target: an operator who typed something
    // outside the allowlist gets the allowlist back immediately, without a
    // registry round-trip and without the host ever being contacted.
    let approved = approve(words)?;
    let target = host_channel::canonical_target(target_name).await?;
    // A program that lives in exactly one place keeps the plain transport: its
    // fixed argv IS the command line, and probing a single path on the host
    // would only replace the remote shell's own report of a missing program
    // with a worse one.
    let account = account_program(approved.argv.first().copied().unwrap_or_default());
    let candidates = match account {
        Some(account) => account.candidates,
        None => approved.candidates(),
    };
    // `mut`: a multi-candidate run reports which path it execed on stderr, and
    // that marker line is consumed out of the operator-visible stderr below.
    let (mut output, used_connection) = match (approved.argv.split_first(), account) {
        (Some((_, arguments)), Some(account)) => {
            let script = account_script(account, arguments);
            host_channel::run_script_with_timeout_and_connection(
                &target,
                &script,
                Duration::from_secs(account.timeout_seconds),
                runner,
            )
            .await?
        }
        (Some(_), None) if approved.argv == PROBIERZ_RUN_ROOT_CREATE => {
            host_channel::run_script_with_timeout_and_connection(
                &target,
                &probierz_run_root_script(),
                host_channel::remote_timeout(),
                runner,
            )
            .await?
        }
        // A read whose fixed paths are relative to the managed account's home
        // stands in that home first. One candidate, one absolute program, so
        // nothing below has a marker to look for.
        (Some(_), None) if home_rooted(approved.argv) => {
            let script = home_rooted_script(approved.argv);
            host_channel::run_script_with_timeout_and_connection(
                &target,
                &script,
                host_channel::remote_timeout(),
                runner,
            )
            .await?
        }
        (Some((_, arguments)), None) if candidates.len() > usize::from(true) => {
            let script = candidate_script(candidates, arguments);
            host_channel::run_script_with_timeout_and_connection(
                &target,
                &script,
                host_channel::remote_timeout(),
                runner,
            )
            .await?
        }
        _ => host_channel::run_program_with_connection(&target, approved.argv, runner).await?,
    };
    // Owned immediately: the route borrows the target it was selected from,
    // and the receipt below moves that target's own fields.
    let used_connection = HostExecConnection::from(used_connection);
    // Which path the host actually execed. Only the multi-candidate script
    // reports it: the account script resolves `$program` in the remote shell
    // and prints no marker, so that path has nothing to report here and
    // asking it for one would fail a run that worked.
    let resolved_executable = if account.is_some() {
        None
    } else if candidates.len() > usize::from(true) {
        match extract_resolved_executable(&mut output.stderr, candidates)? {
            Some(path) => Some(path),
            // A failed run may never have reached any candidate.
            None if !output.ok() => None,
            None => {
                return Err(
                    DeployError("host returned no resolved executable marker".into()).into(),
                )
            }
        }
    } else {
        candidates.first().copied().map(str::to_string)
    };

    let ok = output.ok();
    // Read the remote's own last line before the body is moved into the
    // receipt.
    let error = (!ok).then(|| host_channel::last_error_line(&output, "ssh failed"));
    let receipt = HostExecReceipt {
        schema: "stado.host-exec-receipt.v1".into(),
        target: target.name,
        ssh: target.ssh,
        ssh_fallbacks: target.ssh_fallbacks,
        used_connection,
        command: approved.display(),
        argv: approved
            .argv
            .iter()
            .map(|word| (*word).to_string())
            .collect(),
        program_candidates: (candidates.len() > usize::from(true))
            .then(|| candidates.iter().map(|path| (*path).to_string()).collect()),
        resolved_executable,
        timeout_seconds: account.map(|account| account.timeout_seconds),
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.code,
        status: if ok {
            OK_STATUS.into()
        } else {
            host_channel::FAILED_STATUS.into()
        },
        error,
    };
    serde_json::to_value(receipt).map_err(|error| DeployError(error.to_string()).into())
}
