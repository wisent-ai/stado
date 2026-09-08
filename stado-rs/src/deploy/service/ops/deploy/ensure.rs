use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Ensure: the unit a host must be running, asserted rather than installed
// ---------------------------------------------------------------------------

/// The unit was not there and this pass installed it.
pub const ACTION_CREATED: &str = "created";
/// The unit was there and this pass kicked it in place.
pub const ACTION_RESTARTED: &str = "restarted";
/// The unit was there, running the declared program, and nothing was touched.
pub const ACTION_ALREADY_CORRECT: &str = "already_correct";
/// The unit was there with the declared Program and argv, but its rendered file
/// had drifted; this pass installed and activated the desired definition through
/// the guarded init-system lifecycle. See the incident in [`ensure_service`]:
/// changing `base_unit_environment` to render `HOME` or `STADO_CONFIG` leaves
/// installed units with stale environments until this definition is reloaded.
/// On launchd that requires `bootout` then `bootstrap`, not an in-place kick.
pub const ACTION_CONVERGED: &str = "converged";
/// launchd held a stale program or argument vector and this pass reloaded the
/// already-preflighted definition, then verified launchd's own readback.
pub const ACTION_RELOADED: &str = "reloaded";

/// launchd's system domain: `/Library/LaunchDaemons`, reached with sudo, and
/// the only domain that exists on an ssh login with no Aqua session.
pub const DOMAIN_SYSTEM: &str = "system";
/// The per-login domain (`gui/<uid>` or `user/<uid>`), and `systemd --user`.
pub const DOMAIN_USER: &str = "user";

/// What one `ensure` pass found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureOutcome {
    /// [`ACTION_CREATED`], [`ACTION_RESTARTED`], [`ACTION_ALREADY_CORRECT`],
    /// [`ACTION_CONVERGED`] or [`ACTION_RELOADED`]; any other word is a failure
    /// the remote program named.
    pub action: String,
    /// The domain the unit ended up in, as launchd spells it.
    pub domain: String,
    /// The pid running under the unit after the pass, empty when none is.
    pub pid: String,
    /// The unit file the pass settled on. The body picks the domain, and
    /// therefore the path, after the prelude has already printed the one it
    /// derived, so this is the authority and not `STADO_HOST`'s field.
    pub path: String,
    pub report: RemoteReport,
}

impl EnsureOutcome {
    /// True when the pass reached one of the intended actions AND the host was
    /// observed with the unit loaded and running afterwards.
    ///
    /// [`ACTION_CONVERGED`] belongs here. It was added as a success action —
    /// a drifted unit file rewritten and activated through the guarded
    /// init-system lifecycle — but this set was never widened to admit it, so
    /// every converged pass was reported as
    /// a failure naming the unit path it had just settled on. That is what
    /// stopped the stado 0.13.11 release submission: its "Ensure the declared
    /// object service" step converged
    /// `com.wisent.always-on.stado-object-api` on charless-mac-mini and then
    /// failed with `could not ensure …: converged: /Library/LaunchDaemons/…`,
    /// so no product release could be submitted at all.
    pub fn succeeded(&self) -> bool {
        matches!(
            self.action.as_str(),
            ACTION_CREATED
                | ACTION_RESTARTED
                | ACTION_ALREADY_CORRECT
                | ACTION_CONVERGED
                | ACTION_RELOADED
        ) && self.report.postcondition_held()
    }

    /// The two-valued domain an operator acts on: a unit in the system domain
    /// needs sudo and survives a logout, a unit in the per-login one does
    /// neither. `systemd --user` is the same answer as launchd's per-login
    /// domain, because it is the same fact about who owns the job.
    pub fn domain_word(&self) -> &'static str {
        if self.domain == DOMAIN_SYSTEM {
            DOMAIN_SYSTEM
        } else {
            DOMAIN_USER
        }
    }

    /// True when this pass changed the host.
    pub fn changed(&self) -> bool {
        self.action != ACTION_ALREADY_CORRECT
    }
}

/// The unit file [`ensure_service`] addresses for this plan: the system
/// daemon path when the plan forces daemon placement, and otherwise the empty
/// path, which lets the remote prelude find an existing agent file before it
/// falls back to this login's `LaunchAgents`.
///
/// The distinction is a repair, not a preference. An always-on host that
/// nobody logs into graphically runs its declared services only while their
/// plists sit in `/Library/LaunchDaemons`; the same plist under
/// `~/Library/LaunchAgents` there names a job launchd cannot keep alive,
/// because the per-login domain it loads into exists solely inside sessions
/// nobody opens. `force_daemon` is how the plan carries that host fact
/// ([`requires_daemon_domain`]), and how `service ensure --as-daemon` carries
/// it for a host whose declaration has not caught up yet.
pub fn ensure_unit_path(plan: &DeployPlan) -> String {
    if plan.force_daemon {
        format!("/Library/LaunchDaemons/{}.plist", plan.label)
    } else {
        String::new()
    }
}

/// `service ensure` on one host: leave a matching loaded definition untouched,
/// kick a matching definition when needed, and perform one preflighted
/// definition reload when the on-disk unit or launchd's retained Program or
/// argv differs.
pub async fn ensure_service(
    target: &ComputeTarget,
    plan: &DeployPlan,
    runner: &Runner,
) -> Result<EnsureOutcome, DeployError> {
    // Delimiter first, for the reason [`deploy_service`] gives: substituting
    // it after the unit bodies would let a rendered unit containing the
    // marker text be rewritten into the delimiter itself.
    let body = ENSURE_BODY
        .replace("@HEREDOC@", UNIT_HEREDOC)
        .replace("@PROGRAM@", &shlex_quote(&plan.program))
        .replace("@ARGV@", &shlex_quote(&plan.argv))
        .replace(
            "@DARWIN_DAEMON_UNIT@",
            plan.darwin_daemon_unit.trim_end_matches('\n'),
        )
        .replace("@DARWIN_UNIT@", plan.darwin_unit.trim_end_matches('\n'))
        .replace("@LINUX_UNIT@", plan.linux_unit.trim_end_matches('\n'));
    let prelude = prelude_with(
        &plan.label,
        &plan.unit,
        &ensure_unit_path(plan),
        NO_DOMAIN_SYSTEM,
        None,
    )?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(&plan.label, "ensure");
    let (domain, pid, path) = parse_ensure(&report.stdout)
        .unwrap_or_else(|| (report.domain.clone(), String::new(), report.path.clone()));
    Ok(EnsureOutcome {
        action: report.status.clone(),
        domain,
        pid,
        path,
        report,
    })
}

/// The `STADO_ENSURE` marker: the domain, pid and unit path the body settled
/// on. Absent for every failure path, which is why it is an [`Option`].
fn parse_ensure(stdout: &str) -> Option<(String, String, String)> {
    stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_ENSURE", domain, pid, path] => Some((
                (*domain).to_string(),
                (*pid).to_string(),
                (*path).to_string(),
            )),
            _ => None,
        })
}

/// The managed-service record an `ensure` should be declared under.
///
/// [`record_from_report`] with the ensured path substituted: the record has to
/// name the file the unit was actually installed at, and for a system daemon
/// that is `/Library/LaunchDaemons/<label>.plist` rather than the per-login
/// agent path the prelude derived before the body chose a domain. A record
/// naming a file the host does not have is a declaration no later command can
/// act on.
pub fn record_from_ensure(
    host: &str,
    name: &str,
    outcome: &EnsureOutcome,
    managed_since: &str,
) -> ManagedService {
    let mut record = record_from_report(host, None, name, &outcome.report, managed_since);
    if !outcome.path.is_empty() {
        record.path = outcome.path.clone();
    }
    record
}
