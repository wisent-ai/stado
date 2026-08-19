//! The one ssh channel every read-only `stado host ...` command rides.
//!
//! NO Python original: `docs/missing-commands.md` items 2-6 (`host uptime`,
//! `host ping`, `host disk`, `host cleanup --dry-run`, `host exec`) were
//! never written in Python — the Python CLI stops at `host recover`. The
//! rules below are not new either; they are the shape
//! [`crate::deploy::host_reboot`] already ships, factored out so five
//! commands cannot drift into five slightly different channels:
//!
//! - the canonical remote registry ([`crate::targets::fetch_registry_remote`],
//!   the fleet-survival authority) selects the HOST and nothing else; an
//!   unreachable registry is an error, never an empty registry;
//! - the remote program is FIXED per command and is never assembled from
//!   registry data — registry values reach ssh only as the destination
//!   argument;
//! - the ssh option set is not re-typed here. [`ssh_options`] takes
//!   [`crate::deploy::host_reboot::ssh_reboot_argv`] and drops its trailing
//!   remote program, so `BatchMode=yes`, `ConnectTimeout` and
//!   `StrictHostKeyChecking=accept-new` are literally the same words the
//!   shipped reboot path uses and cannot fall out of step with it;
//! - every subprocess goes through the [`Runner`] seam;
//! - every report carries `exit_code` and a `status` string, and a failure
//!   surfaces the LAST stderr line verbatim.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{
    host_reboot, host_recovery, py_str_repr, shlex_quote, ssh_key, CommandOutput, CommandSpec,
    DeployError, Runner,
};
use crate::targets::{ComputeTarget, Registry};

/// The `status` value every command in this family reports when the remote
/// side did not exit clean. The success value is command-specific.
pub const FAILED_STATUS: &str = "failed";

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
/// [`crate::deploy::host_reboot`] makes, word for word: not in the
/// registry, not a local host, no registry-managed ssh destination.
///
/// One deliberate exception to the last refusal: a target that IS this
/// machine needs no ssh destination, because reaching it does not involve
/// the network. A local pool host with slots and no `ssh` field is a normal
/// registry entry, not a broken one — refusing it made every read-only
/// `stado host ...` command unusable on the box running the command, which
/// is the box an operator most often asks about.
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
    if target.ssh.as_deref().unwrap_or("").is_empty() && !target_is_this_host(target) {
        return Err(DeployError(format!(
            "target {} has no registry-managed ssh destination and is not this host",
            py_str_repr(target_name)
        )));
    }
    Ok(target)
}

/// [`resolve_target`] against the canonical remote registry.
pub async fn canonical_target(target_name: &str) -> Result<ComputeTarget, DeployError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    resolve_target(&registry, target_name).cloned()
}

/// The ssh invocation up to and including the destination, taken from
/// [`crate::deploy::host_reboot::ssh_reboot_argv`] with its one trailing
/// element — the reboot program — removed. Derived rather than re-typed so
/// the option set is provably identical to the shipped one.
pub fn ssh_options(ssh_target: &str) -> Vec<String> {
    let mut argv = host_reboot::ssh_reboot_argv(ssh_target);
    argv.pop();
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

/// Run one fixed program on a resolved target.
///
/// A target that IS this machine runs the program directly. The words are
/// the same compile-time constants the ssh path sends, so the two transports
/// cannot answer different questions; only the hop disappears.
pub async fn run_program(
    target: &ComputeTarget,
    program: &[&str],
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let (argv, _key) = if target_is_this_host(target) {
        (program.iter().map(|word| word.to_string()).collect(), None)
    } else {
        let key = ssh_key::materialize(&target.name).await?;
        let argv = ssh_key::add_identity(
            ssh_program_argv(target.ssh.as_deref().unwrap_or(""), program),
            &key,
        )?;
        (argv, Some(key))
    };
    runner(CommandSpec {
        argv,
        stdin: None,
        timeout: Some(remote_timeout()),
    })
    .await
    .map_err(DeployError)
}

/// Run one fixed script (fed on stdin) on a resolved target.
///
/// The local branch runs the same `/bin/bash -s` the ssh branch asks the
/// login shell for, so the marker protocol on the far side is byte-identical
/// whichever transport carried it.
pub async fn run_script(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_script_with_timeout(target, script, remote_timeout(), runner).await
}

/// Run a fixed remote script with an operation-specific wall-clock bound.
/// Connection setup remains bounded by the shared SSH options.
pub async fn run_script_with_timeout(
    target: &ComputeTarget,
    script: &str,
    timeout: Duration,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let (argv, _key) = if target_is_this_host(target) {
        (vec!["/bin/bash".to_string(), "-s".to_string()], None)
    } else {
        let key = ssh_key::materialize(&target.name).await?;
        let argv =
            ssh_key::add_identity(ssh_script_argv(target.ssh.as_deref().unwrap_or("")), &key)?;
        (argv, Some(key))
    };
    runner(CommandSpec {
        argv,
        stdin: Some(script.to_string()),
        timeout: Some(timeout),
    })
    .await
    .map_err(DeployError)
}

/// Run one FIXED script on a named target and hand back what it printed.
///
/// For a caller that wants the remote's answer rather than a report to
/// display: `stado identity verify` asks a host which of its users hold Apple
/// accounts, and needs the lines, not a rendering of them.
///
/// The script is a compile-time constant of the calling module, embedded in
/// this binary — this is the channel the retired helper-install-and-run pair
/// used to be the long way around. The program travels with stado itself, so
/// there is nothing to install on the host, nothing left behind after the
/// read, and nothing an operator can point at a different program.
pub async fn run_fixed_script(
    target_name: &str,
    script: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let target = canonical_target(target_name).await?;
    let output = run_script(&target, script, runner).await?;
    if !output.ok() {
        return Err(DeployError(last_error_line(
            &output,
            "the remote read did not complete",
        )));
    }
    Ok(output.stdout)
}

/// The `target` / `ssh` head every report in this family opens with,
/// identical to the one `host reboot` emits.
pub fn base_report(target: &ComputeTarget) -> Map<String, Value> {
    let mut report = Map::new();
    report.insert("target".to_string(), json!(target.name));
    report.insert(
        "ssh".to_string(),
        target.ssh.as_ref().map_or(Value::Null, |ssh| json!(ssh)),
    );
    report
}

/// The last line of the remote failure detail, verbatim.
///
/// Whatever actually went wrong — sudo asking for a password, a missing
/// binary, a refused key — the operator needs the remote's own words, not
/// a paraphrase. `fallback` covers a failure that produced no output at all.
pub fn last_error_line(output: &CommandOutput, fallback: &str) -> String {
    let detail = output.detail().trim();
    match detail.lines().next_back() {
        Some(line) => line.to_string(),
        None => fallback.to_string(),
    }
}

/// Close a report the way `host reboot` closes its own: `exit_code`, a
/// `status` string, and on failure the last stderr line under `error`.
pub fn finish_report(
    report: &mut Map<String, Value>,
    output: &CommandOutput,
    ok_status: &str,
    fallback_error: &str,
) {
    report.insert("exit_code".to_string(), json!(output.code));
    report.insert(
        "status".to_string(),
        json!(if output.ok() {
            ok_status
        } else {
            FAILED_STATUS
        }),
    );
    if !output.ok() {
        report.insert(
            "error".to_string(),
            json!(last_error_line(output, fallback_error)),
        );
    }
}

/// Split one marker line into its tab-separated fields.
///
/// Every remote script in this family speaks the tab-delimited `STADO_*`
/// marker protocol of [`crate::deploy::host_recovery::parse_output`]; the
/// parsers match on the resulting slice, so an unexpected field count
/// falls through instead of panicking on an index.
pub fn marker_fields(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}

// ---------------------------------------------------------------------------
// Declared end states
// ---------------------------------------------------------------------------

/// The marker a postcondition probe prints, in the same tab-delimited family
/// as every `STADO_*` marker on this channel.
pub const POSTCONDITION_MARKER: &str = "POSTCONDITION";

/// The probe found the host in the state the operation said it would leave
/// behind.
pub const POSTCONDITION_MET: &str = "met";
/// The probe ran and the host is NOT in that state.
pub const POSTCONDITION_UNMET: &str = "unmet";
/// No verdict line came back at all, so the end state was never observed.
/// Deliberately not folded into [`POSTCONDITION_UNMET`]: "the host says no"
/// and "nobody asked the host" are different facts, and only one of them
/// tells an operator where to look.
pub const POSTCONDITION_UNOBSERVED: &str = "unobserved";

/// The end state a host operation intends, and the probe that checks it on
/// the host itself.
///
/// `stado service restart` on the always-on Mac booted a unit out, could not
/// bootstrap it back because launchd still held children of the old job,
/// reported `restart_failed: disowned process survived` and left the unit
/// UNLOADED. Every step of that script did exactly what it was written to
/// do. Nothing at all asked whether the machine had ended up in the state
/// the operator asked for, so the one fact that mattered — the listeners are
/// gone — was the one fact no report carried. An operation that states its
/// intended end state and never compares it against the world is a
/// declaration, not a check.
///
/// `describe` is the intent in the operator's words ("the unit is loaded and
/// has a pid"), and doubles as the probe's identity in the output so a line
/// printed by some other program cannot be read as this operation's verdict.
/// `probe` is a POSIX sh fragment that prints exactly
/// `POSTCONDITION\t<describe>\t<met|unmet>\t<detail>`; it emits that through
/// the `stado_post` helper [`PostCondition::arm`] defines, which is `say`
/// with a different marker word, because a second output convention on this
/// channel would be a second parser to keep in step.
pub struct PostCondition {
    /// The intended end state, in the operator's words.
    pub describe: &'static str,
    /// POSIX sh that observes the host and emits one verdict line.
    pub probe: String,
}

impl PostCondition {
    /// The shell that defines the verdict emitter and arms the probe.
    ///
    /// Armed as an `EXIT` trap and spliced in BEFORE the operation body
    /// rather than appended after it, for two reasons that are not
    /// stylistic. Every lifecycle body on this channel exits the moment it
    /// has something to say — `say 'restarted' "$domain in place"; exit 0`
    /// is the FIRST branch of the restart script — so a fragment appended
    /// after the body would run on every path except the ones that claim
    /// success, which are precisely the paths that lied. And a trap runs in
    /// the operation's own shell, so it observes the domain and unit the
    /// body finally settled on rather than the ones it started with; the
    /// restart fallback renames `$unit` to the `-recovery` label, and a
    /// probe reading the original label would call a healthy host broken.
    ///
    /// The trap hands back the status that triggered it, so declaring an
    /// end state cannot change the exit code the transport reports.
    fn arm(&self) -> String {
        let describe = shlex_quote(self.describe);
        let probe = &self.probe;
        format!(
            "stado_post() {{
  pc_detail=$(printf '%s' \"$2\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf '{POSTCONDITION_MARKER}\\t%s\\t%s\\t%s\\n' {describe} \"$1\" \"$pc_detail\"
}}
stado_postcondition() {{
  pc_rc=$?
{probe}  return $pc_rc
}}
trap stado_postcondition EXIT
"
        )
    }
}

/// What the host said about the end state an operation promised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostConditionVerdict {
    /// The intended end state, echoed back so a report can print the intent
    /// beside the outcome instead of a bare `unmet`.
    pub describe: String,
    /// [`POSTCONDITION_MET`], [`POSTCONDITION_UNMET`] or
    /// [`POSTCONDITION_UNOBSERVED`].
    pub state: String,
    /// The probe's own words about what it found.
    pub detail: String,
}

/// Read one probe verdict out of an operation's stdout.
///
/// A verdict for some other end state is ignored, and a missing verdict is
/// [`POSTCONDITION_UNOBSERVED`] rather than an optimistic default: an
/// unobserved end state is exactly the thing this channel is no longer
/// allowed to assume in the operation's favour.
pub fn postcondition_verdict(stdout: &str, postcondition: &PostCondition) -> PostConditionVerdict {
    for line in stdout.lines() {
        if let [POSTCONDITION_MARKER, describe, state, detail] = marker_fields(line).as_slice() {
            if *describe == postcondition.describe {
                return PostConditionVerdict {
                    describe: (*describe).to_string(),
                    state: (*state).to_string(),
                    detail: (*detail).to_string(),
                };
            }
        }
    }
    PostConditionVerdict {
        describe: postcondition.describe.to_string(),
        state: POSTCONDITION_UNOBSERVED.to_string(),
        detail: "the host returned no verdict for this end state".to_string(),
    }
}

/// Run a fixed remote script that has to leave the host in a declared state,
/// and bring back both halves of the answer: what the operation said about
/// itself, and what the host said about the state the operation promised.
///
/// `head` is the operation's vocabulary (the caller's prelude, which the
/// probe reads its unit, domain and init-system helpers from) and `body` is
/// the operation. Assembling the two here, rather than letting each caller
/// concatenate its own, is what keeps the arming order — and therefore the
/// guarantee that the probe survives an early `exit` — in one place.
pub async fn run_checked_script(
    target: &ComputeTarget,
    head: &str,
    body: &str,
    postcondition: &PostCondition,
    runner: &Runner,
) -> Result<(CommandOutput, PostConditionVerdict), DeployError> {
    let script = format!("{head}{}{body}", postcondition.arm());
    let output = run_script(target, &script, runner).await?;
    let verdict = postcondition_verdict(&output.stdout, postcondition);
    Ok((output, verdict))
}
