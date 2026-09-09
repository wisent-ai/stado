use crate::deploy::service::*;

/// One process the reaper judged, and what it did about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedProcess {
    pub host: String,
    pub pid: String,
    /// `would_end`, `ended`, `still_running`, or `kept`.
    ///
    /// `kept` is a row the reaper refuses to signal because a declared label
    /// holds that pid or one of its ancestors. It is reported for the same
    /// reason the others are: naming what a declared label is actually
    /// running is how a stale binary that survived a delivery becomes
    /// visible, and the keep-set is exactly where such a process hides.
    pub outcome: String,
    pub started_at: String,
    pub command: String,
}

impl ReapedProcess {
    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "pid": self.pid,
            "outcome": self.outcome,
            "started_at": self.started_at,
            "command": self.command,
        })
    }
}

/// [`REAP_SCRIPT`] against one host. `apply` false signals nothing.
pub async fn reap_undeclared_processes(
    target: &ComputeTarget,
    command_match: &str,
    apply: bool,
    runner: &Runner,
) -> Result<(Vec<ReapedProcess>, String, Vec<String>, Vec<Value>), DeployError> {
    if command_match.trim().is_empty() {
        return Err(DeployError(
            "a command substring is required: the reaper de-duplicates one named program, never \
             everything under a managed root"
                .to_string(),
        ));
    }
    let mut roots = Vec::new();
    for root in managed_roots()? {
        roots.push(format!("\"{}\"", quote_unit_path(&root)?));
    }
    let mut labels = Vec::new();
    for service in declared_services(target) {
        labels.push(format!("\"{}\"", quote_unit_path(service.unit_id())?));
    }
    let script = REAP_SCRIPT
        .replace("@ROOTS@", &roots.join(" "))
        .replace("@LABELS@", &labels.join(" "))
        .replace("@APPLY@", if apply { "yes" } else { "no" })
        .replace(
            "@MATCH@",
            &format!("\"{}\"", quote_command_match(command_match)?),
        );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the reap did not complete",
        )));
    }
    let mut kept = String::new();
    let mut reaped = Vec::new();
    let mut scanned_roots = Vec::new();
    let mut examined = Vec::new();
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_REAP_ROOT", root] => scanned_roots.push(root.to_string()),
            ["STADO_REAP_SCAN", root, code, output] => examined.push(json!({
                "operation": "pgrep", "root": root, "exit_code": code, "output": output
            })),
            ["STADO_REAP_EXAMINED", pid, root, command] => examined.push(json!({
                "pid": pid, "root": root, "command": command
            })),
            ["STADO_REAP_KEEP", pids] => kept = (*pids).trim().to_string(),
            ["STADO_REAP", pid, outcome, started, command] => reaped.push(ReapedProcess {
                host: target.name.clone(),
                pid: (*pid).trim().to_string(),
                outcome: (*outcome).trim().to_string(),
                started_at: (*started).trim().to_string(),
                command: (*command).trim().to_string(),
            }),
            _ => {}
        }
    }
    Ok((reaped, kept, scanned_roots, examined))
}

/// Every launchd job loaded on TARGET that the registry does not declare, with
/// the unit file and program each one runs.
///
/// This is the direction nothing in the product looked. `service list` walks
/// the declaration and asks the host about each entry; `list --unowned` walks
/// the processes and asks launchd who owns them — and on the always-on mac it
/// correctly answered that nothing is unowned, because every duplicate IS
/// owned, by a label the registry never heard of.
///
/// charless-mac-mini was running three queue agents at once under that blind
/// spot: `com.wisent.compute.service.stado-agent-mini`, the only one the
/// registry declares, plus `com.wisent.compute.agent.charless-mac-mini` from
/// `stado bootstrap --local`'s label convention and
/// `com.wisent.compute.service.stado-queue-agent` from a third. All three
/// published capacity for the same consumer id, so the oldest binary on the
/// box decided what the host answered, and 55 pinned jobs were refused for
/// seven days by a process no report could name.
///
/// Scope is the registry's declaration and nothing else. This used to be
/// "every job under [`FLEET_LABEL_PREFIX`] that the registry does not declare",
/// and the prefix was the entire hiding place: `com.stado.agent.charless-mac-mini`
/// was loaded on that same host on 2026-09-01, was the only label on it outside
/// the prefix, held the pid overwriting the janitor's state file — and this
/// function answered that the host had no undeclared unit. Callers that want to
/// treat an out-of-prefix row differently read
/// [`UndeclaredUnit::classification`]; nothing decides that by filtering.
pub async fn undeclared_units(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UndeclaredUnit>, DeployError> {
    Ok(loaded_units(target, runner)
        .await?
        .into_iter()
        .filter(|unit| !unit.declared)
        .collect())
}

/// EVERY launchd label loaded on TARGET, plus every label with a unit file in
/// the three directories this fleet installs into — declared or not, fleet-named
/// or not, with every domain that declares it and the registry's verdict on each.
///
/// [`undeclared_units`] is this list minus the rows the registry declares, and
/// that subtraction is why one class of duplicate hid for a whole evening: a
/// label declared once as a system LaunchDaemon and once as a user LaunchAgent
/// is DECLARED, so it never appears in the undeclared view, while launchd runs
/// both copies. Three processes served one declared port on the always-on mac
/// behind exactly that. Callers that need to reason about duplication read this
/// one; callers that need to reason about ownership read the other.
///
/// Neither list is filtered by label any more. This function used to drop every
/// row outside [`FLEET_LABEL_PREFIX`] before returning, so the sweep and the
/// undeclared view were both blind to the same set, and the one job on
/// charless-mac-mini that mattered on 2026-09-01 was in it. A check that wants
/// a narrower population states that narrowing itself, from evidence it holds,
/// and says so where an operator can read it.
pub async fn loaded_units(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UndeclaredUnit>, DeployError> {
    Ok(loaded_units_with_posture(target, runner).await?.0)
}
