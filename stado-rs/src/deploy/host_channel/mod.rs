//! The one registry-authorized channel every host capability rides.
//!
//! Host and space operations share the option set and report shape factored
//! from [`crate::deploy::host_state::reboot`], so target identity, local-vs-SSH
//! selection, timeout behavior, and failure sentences cannot drift into
//! capability-specific variants:
//!
//! - [`canonical_registry`] selects the HOST and nothing else: the canonical
//!   remote registry ([`crate::targets::fetch_registry_remote`], the
//!   fleet-survival authority), or the last-known-good copy with its age
//!   announced once when the store does not answer. Never an empty registry,
//!   and never a guessed host — an unknown name still fails;
//! - the remote program is FIXED per command and is never assembled from
//!   registry data — registry values reach ssh only as the destination
//!   argument;
//! - the ssh option set is not re-typed here. [`ssh_options`] takes
//!   [`crate::deploy::host_state::reboot::ssh_reboot_argv`] and drops its trailing
//!   remote program, so `BatchMode=yes`, `ConnectTimeout` and
//!   `StrictHostKeyChecking=accept-new` are literally the same words the
//!   shipped reboot path uses and cannot fall out of step with it;
//! - every subprocess goes through the [`Runner`] seam;
//! - every report carries `exit_code` and a `status` string, and a failure
//!   surfaces the LAST stderr line verbatim.

use std::future::Future;
use std::time::Duration;

use super::{host_state::reboot, host_recovery, py_str_repr, shlex_quote, host_access::ssh_key, DeployError, Runner};
use crate::targets::{ComputeTarget, Registry};

/// The `status` value every command in this family reports when the remote
/// side did not exit clean. The success value is command-specific.
pub const FAILED_STATUS: &str = "failed";
const CONNECTION_PROBE_PROGRAM: [&str; 1] = ["true"];
const CONNECTION_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

struct HostSession {
    target: String,
    key: ssh_key::KeyFile,
    connection_name: String,
    destination: String,
}

tokio::task_local! {
    static HOST_SESSION: HostSession;
}

pub(super) fn session_key(target: &str) -> Option<ssh_key::KeyFile> {
    HOST_SESSION
        .try_with(|session| (session.target == target).then(|| session.key.clone()))
        .ok()
        .flatten()
}

/// Authenticate and choose a declared route once for a multi-command operation.
/// The key and route live only as long as this future; failed commands are never
/// replayed on another route.
pub(crate) async fn with_session<T>(
    target: &ComputeTarget,
    runner: &Runner,
    operation: impl Future<Output = Result<T, DeployError>>,
) -> Result<T, DeployError> {
    if target_is_this_host(target) {
        return operation.await;
    }
    let key = ssh_key::materialize(target.channel_key()).await?;
    let connection = select_connection_with_key(target, &key, runner).await?;
    let session = HostSession {
        target: target.name.clone(),
        key,
        connection_name: connection.name.to_string(),
        destination: connection.destination.to_string(),
    };
    HOST_SESSION.scope(session, operation).await
}

/// One declared route for the SSH host-control transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshConnection<'a> {
    pub name: &'a str,
    pub destination: &'a str,
}

/// The result of a side-effect-free authentication probe on one route.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SshConnectionProbe {
    pub name: String,
    pub destination: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The route that carried one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsedConnection<'a> {
    Local,
    Ssh(SshConnection<'a>),
}

/// True when the registry entry names THIS machine, matched the way the
/// registry matches identities elsewhere: case-insensitively, on the short
/// name as well as the fully qualified one.
///
/// Lifted out of [`crate::deploy::host_build_caches`], which had the only
/// copy, so the read-only host channel and the cache pass cannot disagree
/// about which box they are standing on.
pub fn target_is_this_host(target: &ComputeTarget) -> bool {
    let hostname = crate::providers::vast::system_hostname().to_lowercase();
    if hostname.is_empty() {
        return false;
    }
    let short = hostname.split('.').next().unwrap_or_default().to_string();
    target.hostnames.iter().any(|candidate| {
        let candidate = candidate.to_lowercase();
        candidate == hostname
            || candidate == short
            || candidate.split('.').next().unwrap_or_default() == short
    })
}

/// Registry-authorized host resolution, with the refusals
/// [`crate::deploy::host_state::reboot`] makes, word for word: not in the
/// registry, not a local host, no registry-managed ssh destination.
///
/// One deliberate exception to the last refusal: a target that IS this
/// machine needs no ssh destination, because reaching it does not involve
/// the network. A local compute host with no `ssh` field is a normal registry
/// entry, not a broken one — refusing it made every read-only `stado host ...`
/// command unusable on the box running the command.
pub fn resolve_target<'a>(
    registry: &'a Registry,
    target_name: &str,
) -> Result<&'a ComputeTarget, DeployError> {
    let Some(target) = registry.lookup(target_name) else {
        return Err(DeployError(format!(
            "target {} is not in the canonical registry",
            py_str_repr(target_name)
        )));
    };
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(target_name)
        )));
    }
    if !target.has_ssh_connection() && !target_is_this_host(target) {
        return Err(DeployError(format!(
            "target {} has no registry-managed ssh destination and is not this host",
            py_str_repr(target_name)
        )));
    }
    Ok(target)
}

/// The registry every host-channel operation resolves through: the canonical
/// store first, the last-known-good copy — with its age on stderr, once —
/// when the store does not answer.
///
/// A host command that cannot resolve its own host while the registry store
/// is unreachable goes silent exactly when the fleet does: on 2026-08-19
/// every `stado host ...` invocation against a mac mini that had dropped off
/// the network failed with one line about the store, and the question — which
/// host went quiet, and when — went unanswered because no reader would speak
/// without the authority. The copy is not an empty registry and never invents
/// a target: an unknown name still fails, and [`resolve_target`]'s refusals
/// are unchanged.
pub async fn canonical_registry() -> Result<Registry, DeployError> {
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    if let Some(notice) = notice {
        crate::targets::report_registry_notice(&notice);
    }
    Ok(registry)
}

/// [`resolve_target`] against [`canonical_registry`].
pub async fn canonical_target(target_name: &str) -> Result<ComputeTarget, DeployError> {
    resolve_target(&canonical_registry().await?, target_name).cloned()
}

/// The ssh invocation up to and including the destination, taken from
/// [`crate::deploy::host_state::reboot::ssh_reboot_argv`] with its one trailing
/// element — the reboot program — removed. Derived rather than re-typed so
/// the option set is provably identical to the shipped one.
pub fn ssh_options(ssh_target: &str) -> Vec<String> {
    let mut argv = reboot::ssh_reboot_argv(ssh_target);
    argv.pop();
    if tracing::enabled!(
        target: "stado::deploy::host_channel",
        tracing::Level::TRACE
    ) {
        argv.insert(argv.len() - 1, "-vvv".to_string());
    }
    argv
}

/// ssh argv running one FIXED remote program.
///
/// ssh joins everything after the destination with spaces and hands the
/// result to the login shell, so the words are quoted for that shell here
/// (Python `shlex.quote`) instead of being passed as separate ssh
/// arguments. The words are compile-time constants of the calling module;
/// no caller may route registry data or operator input through here.
pub fn ssh_program_argv(ssh_target: &str, program: &[&str]) -> Vec<String> {
    let mut argv = ssh_options(ssh_target);
    argv.push(
        program
            .iter()
            .map(|word| shlex_quote(word))
            .collect::<Vec<String>>()
            .join(" "),
    );
    argv
}

/// ssh argv running a fixed remote script fed on stdin — the transport
/// [`crate::deploy::host_recovery::ssh_argv`] uses for its marker protocol.
pub fn ssh_script_argv(ssh_target: &str) -> Vec<String> {
    let mut argv = ssh_options(ssh_target);
    argv.push("/bin/bash".to_string());
    argv.push("-s".to_string());
    argv
}

/// The wall-clock cap on a remote read.
///
/// One channel, one cap: [`crate::deploy::host_recovery::TIMEOUT_SECONDS`],
/// already the ceiling for the heaviest thing this fleet runs over ssh (the
/// recovery pass, its cleanup included). The connect half is bounded far
/// tighter by the inherited `ConnectTimeout` option, so a dead box still
/// fails fast.
pub fn remote_timeout() -> Duration {
    Duration::from_secs(host_recovery::TIMEOUT_SECONDS)
}

mod connection;
mod postcondition;
mod report;
mod run;

pub(in crate::deploy::host_channel) use connection::select_connection_with_key;
pub use connection::{probe_ssh_connections, select_ssh_connection};
pub use postcondition::{
    postcondition_verdict, run_checked_script, PostCondition, PostConditionVerdict,
    POSTCONDITION_MARKER, POSTCONDITION_MET, POSTCONDITION_UNMET, POSTCONDITION_UNOBSERVED,
};
pub use report::{base_report, finish_report, last_error_line, marker_fields};
pub use run::{
    extract_semver, remote_home, remote_json_member, remote_program_version, remote_read_file,
    remote_test, run_command, run_program, run_program_with_connection, run_program_with_stdin,
    run_program_with_stdin_and_connection, run_program_with_timeout, run_script,
    run_script_with_timeout, run_script_with_timeout_and_connection,
};
