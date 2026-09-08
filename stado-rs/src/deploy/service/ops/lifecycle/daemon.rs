use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Write side: one command per remote program
// ---------------------------------------------------------------------------

/// Report the argument vector a managed unit runs, exactly as declared.
pub async fn show_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, SHOW_BODY)?;
    run_remote(target, script, runner).await
}

// ---------------------------------------------------------------------------
// The one repair a system LaunchDaemon has that needs no privilege
// ---------------------------------------------------------------------------

/// `KeepAlive` is `<true/>`: launchd recreates the process whenever it ends,
/// for any reason. This is the only spelling that authorizes ending the
/// process, because it is the only one under which the answer to "will
/// something put it back" is yes without reading further keys.
pub const KEEP_ALIVE_ALWAYS: &str = "true";
/// `KeepAlive` is a dict (`SuccessfulExit`, `Crashed`, `PathState`, ...).
/// launchd may or may not respawn after a signal depending on those keys,
/// and guessing which is not a thing to do to a control plane.
pub const KEEP_ALIVE_CONDITIONAL: &str = "conditional";
/// The unit declares no `KeepAlive` at all.
pub const KEEP_ALIVE_ABSENT: &str = "absent";
/// The plist could not be read, so nothing about respawning is known.
pub const KEEP_ALIVE_UNREADABLE: &str = "unreadable";

/// What one system LaunchDaemon looks like from the approved unprivileged
/// login: its respawn declaration, this login's account, and which of the
/// pids running its program that account owns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemDaemon {
    /// [`KEEP_ALIVE_ALWAYS`], [`KEEP_ALIVE_CONDITIONAL`],
    /// [`KEEP_ALIVE_ABSENT`], [`KEEP_ALIVE_UNREADABLE`], or the literal
    /// scalar the plist carries (`false` is the one that matters).
    pub keep_alive: String,
    /// The account the approved channel logs in as.
    pub login_user: String,
    /// Pids running exactly the argv this unit declares, that
    /// [`Self::login_user`] owns and can therefore signal without privilege.
    pub owned_pids: Vec<String>,
    /// Pids running it that some other account owns.
    pub foreign_pids: Vec<String>,
    /// The whole argument vector the unit declares, single-spaced.
    ///
    /// The argv and not the program: every Stado service on control-host
    /// runs `/Users/charles/.stado/bin/stado`, so the program is the fleet and
    /// the argv is the unit. A restart that resolved its pids by program TERMed
    /// eight processes there on 2026-08-19 and reported one unit restarted.
    pub argv: String,
}

impl SystemDaemon {
    /// True when launchd will unconditionally put a new process in place of
    /// one that ends.
    pub fn respawns(&self) -> bool {
        self.keep_alive == KEEP_ALIVE_ALWAYS
    }

    /// True when this login can perform the whole restart on its own: the
    /// process is one it owns, and launchd is keeping the job alive.
    pub fn restartable_unprivileged(&self) -> bool {
        self.respawns() && !self.owned_pids.is_empty()
    }

    /// Why this daemon cannot be restarted from here, in the operator's
    /// words. Only reached when [`Self::restartable_unprivileged`] is false,
    /// and it always names the privileged command that does work.
    pub(super) fn refusal(&self, service: &ManagedService) -> String {
        let reason = if !self.respawns() {
            match self.keep_alive.as_str() {
                KEEP_ALIVE_ABSENT => "the unit declares no KeepAlive, so ending its process would \
                                      leave nothing to start another one and this host would go \
                                      from degraded to down"
                    .to_string(),
                KEEP_ALIVE_CONDITIONAL => "the unit declares a conditional KeepAlive, so whether \
                                           launchd respawns it after a signal depends on keys \
                                           this channel must not guess at"
                    .to_string(),
                KEEP_ALIVE_UNREADABLE => "the unit's plist could not be read, so whether anything \
                                          would start another process is unknown"
                    .to_string(),
                other => format!(
                    "the unit declares KeepAlive {other}, so launchd will not start another \
                     process when this one ends"
                ),
            }
        } else if !self.foreign_pids.is_empty() {
            format!(
                "its process runs as another account (pid(s) {}), not as the approved user {}, so \
                 this channel cannot signal it",
                self.foreign_pids.join(" "),
                self.login_user
            )
        } else {
            format!(
                "nothing on the host is running {}, so there is no process to end and launchd is \
                 not holding the job up",
                if self.argv.is_empty() {
                    "the unit's declared argv"
                } else {
                    &self.argv
                }
            )
        };
        format!(
            "{} on {} is a system LaunchDaemon at {}; the approved channel is unprivileged and \
             cannot bootstrap it, and {reason}. Restarting it needs one privileged command on the \
             host: sudo launchctl kickstart -k system/{}",
            service.unit_id(),
            service.host,
            service.path,
            service.unit_id()
        )
    }
}

/// The `STADO_DAEMON` marker. Absent for every path that never reached the
/// probe (a missing unit file, an unsupported OS), which is why it is an
/// [`Option`].
fn parse_daemon(stdout: &str) -> Option<SystemDaemon> {
    let words = |field: &str| -> Vec<String> {
        field
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    for line in stdout.lines() {
        if let ["STADO_DAEMON", keep_alive, login_user, owned, foreign, argv] =
            host_channel::marker_fields(line).as_slice()
        {
            return Some(SystemDaemon {
                keep_alive: (*keep_alive).to_string(),
                login_user: (*login_user).to_string(),
                owned_pids: words(owned),
                foreign_pids: words(foreign),
                argv: (*argv).to_string(),
            });
        }
    }
    None
}

/// The pids the terminate program may signal, as one shell word list.
///
/// Every value here was reported by the host's own `pgrep` moments ago, but
/// it still travels back over the channel as data, and a signal list is the
/// last place to trust a round trip. Digits and single spaces only; anything
/// else is refused rather than quoted, because the useful failure is "the
/// host said something this operation does not understand", never a
/// creatively escaped `kill` argument.
pub(crate) fn validate_pid_list(pids: &[String]) -> Result<String, DeployError> {
    for pid in pids {
        if pid.is_empty() || !pid.chars().all(|character| character.is_ascii_digit()) {
            return Err(DeployError(format!(
                "the host reported {} as a process id of this unit, which is not a process id",
                py_str_repr(pid)
            )));
        }
    }
    Ok(pids.join(" "))
}

/// Read one system LaunchDaemon's respawn declaration and process ownership.
///
/// Read-only: it starts nothing, stops nothing and signals nothing, so it is
/// safe against a live production host.
pub async fn inspect_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<(RemoteReport, Option<SystemDaemon>), DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, DAEMON_PROBE_BODY)?;
    let report = run_remote(target, script, runner).await?;
    let daemon = parse_daemon(&report.stdout);
    Ok((report, daemon))
}
