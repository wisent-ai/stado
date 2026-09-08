use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Declared domain against the domain the host can have
// ---------------------------------------------------------------------------

/// The `role` / `host_heuristic` word for a host that is expected to serve
/// with nobody sitting at it. `control-host` carries it in both fields.
pub const ROLE_ALWAYS_ON: &str = "always-on";

/// Does the registry say this host is meant to keep a graphical account alive?
///
/// `always-on` describes uptime, not the absence of a login. The Mac mini is
/// both always-on and the declared Weles host; autologin keeps its Aqua domain
/// alive so browser-facing LaunchAgents can run there. Treating uptime as
/// headlessness moved those jobs into the system domain, where they competed
/// with the release-owned user jobs for the same ports.
pub fn declared_graphical(target: &ComputeTarget) -> bool {
    target.weles.as_ref().is_some_and(|policy| policy.enabled) || target.display_stream.is_some()
}

/// Does the registry itself say this host runs unattended?
pub fn declared_always_on(target: &ComputeTarget) -> bool {
    [target.role.as_deref(), target.host_heuristic.as_deref()]
        .into_iter()
        .flatten()
        .any(|word| word == ROLE_ALWAYS_ON)
}

/// Is the system domain the only declared launchd domain for this host?
///
/// An always-on Darwin target defaults to system services only when the same
/// declaration does not assign it a persistent graphical workload. Linux uses
/// systemd user lingering and does not have launchd domains.
pub fn requires_daemon_domain(target: &ComputeTarget) -> bool {
    declared_always_on(target)
        && !declared_graphical(target)
        && !target.release_platform.starts_with("linux")
}

/// Where a launchd job that belongs to the machine lives.
const DAEMON_DIR: &str = "/Library/LaunchDaemons";
/// The `/Users/<account>/...` prefix a per-account agent path carries. The
/// account is load-bearing: a LaunchAgent's job runs as its owner, and the
/// daemon spelling of the same unit only keeps running as that owner if it
/// carries `UserName` (`local_install::daemon_plist_text`).
const ACCOUNTS_PREFIX: &str = "/Users/";

/// A unit declared in a launchd domain the host it is declared on cannot
/// have.
///
/// `com.wisent.compute.service.stado-agent-mini` was declared as a user
/// LaunchAgent at `/Users/charles/Library/LaunchAgents/...` on
/// `control-host`, a host declared always-on in both `role` and
/// `host_heuristic` and with no graphical session at all: `/dev/console` is
/// root's, `who` prints nothing, and the login's own `launchctl list` holds
/// no `com.wisent.*` label. `launchctl bootstrap user/501 <plist>` answers
/// `Bootstrap failed: 5: Input/output error` there and `gui/501` does not
/// exist, so the declaration named a domain that could never load it. Every
/// other always-on unit on that host is a system LaunchDaemon under
/// [`DAEMON_DIR`].
///
/// The declaration is checkable without going anywhere: the path says the
/// domain and the target says the host runs unattended. So this is a
/// registry finding, reported by `stado registry doctor` and printed under
/// `stado service list`, rather than a surprise the next `restart` produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisdeclaredDomain {
    /// Registry target the unit is declared on.
    pub host: String,
    /// The host's own name for the unit — the launchd label.
    pub unit: String,
    /// The unit-file path the declaration carries.
    pub path: String,
    /// The domain that path places the unit in, as [`UnitDomain::as_str`]
    /// spells it.
    pub declared_domain: &'static str,
    /// The only domain this host can load a unit into.
    pub loadable_domain: &'static str,
    /// Where the daemon spelling of this unit belongs.
    pub daemon_path: String,
    /// The account the agent's job runs as, read out of the declared path;
    /// empty for a machine-wide `/Library/LaunchAgents` declaration, which
    /// names no account at all.
    pub account: String,
}

impl MisdeclaredDomain {
    /// The finding for one declared unit, or `None` when the declaration and
    /// the host agree.
    ///
    /// Registry-declared units only. A `host_recovery::MANAGED_AGENTS` entry
    /// is carried by that fixed program and not by the document, so it is
    /// not a registry finding and correcting the document would not move it.
    pub fn detect(target: &ComputeTarget, service: &ManagedService) -> Option<Self> {
        if service.source != SOURCE_REGISTRY || !requires_daemon_domain(target) {
            return None;
        }
        let declared = UnitDomain::from_path(&service.path);
        if !declared.is_per_login() {
            return None;
        }
        let unit = service.unit_id().to_string();
        let account = service
            .path
            .strip_prefix(ACCOUNTS_PREFIX)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default()
            .to_string();
        Some(Self {
            host: target.name.clone(),
            daemon_path: format!("{DAEMON_DIR}/{unit}.plist"),
            unit,
            path: service.path.clone(),
            declared_domain: declared.as_str(),
            loadable_domain: DOMAIN_SYSTEM,
            account,
        })
    }

    /// The privileged command that puts this unit in the domain the host can
    /// load, spelled the way `ENSURE_BODY` installs a daemon
    /// (`install -m 644 -o root -g wheel`) so the file an operator writes by
    /// hand and the file the fleet writes have the same owner and mode.
    ///
    /// `UserName` rides along wherever the declared path names an account:
    /// root reads a plist in [`DAEMON_DIR`], and a daemon without that key
    /// would run the account's program as uid 0 against an account-owned
    /// `~/.stado` — the exact trade `local_install::daemon_plist_text`
    /// documents.
    pub fn install_command(&self) -> String {
        let install = format!(
            "/usr/bin/install -m 644 -o root -g wheel {} {}",
            self.path, self.daemon_path
        );
        if self.account.is_empty() {
            return format!("sudo {install}");
        }
        format!(
            "sudo /bin/sh -c '{install} && /usr/bin/plutil -insert UserName -string {} {}'",
            self.account, self.daemon_path
        )
    }

    /// The one sentence both surfaces print: the unit, the domain it
    /// declares, the domain the host can actually load, and the command that
    /// closes the gap.
    pub fn sentence(&self) -> String {
        format!(
            "{} is declared in launchd's {} domain ({}), and {} is declared {ROLE_ALWAYS_ON}, so no \
             account is logged in graphically there, launchd builds no gui/<uid>, and {} is the only \
             domain that host can load a unit into; install it there with one privileged command on \
             the host: {}",
            self.unit,
            self.declared_domain,
            self.path,
            self.host,
            self.loadable_domain,
            self.install_command()
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "unit": self.unit,
            "path": self.path,
            "declared_domain": self.declared_domain,
            "loadable_domain": self.loadable_domain,
            "daemon_path": self.daemon_path,
            "install_command": self.install_command(),
            "detail": self.sentence(),
        })
    }
}

/// Every registry-declared unit on TARGET whose declared launchd domain the
/// host cannot have.
pub fn misdeclared_domains(target: &ComputeTarget) -> Vec<MisdeclaredDomain> {
    declared_services(target)
        .iter()
        .filter_map(|service| MisdeclaredDomain::detect(target, service))
        .collect()
}
