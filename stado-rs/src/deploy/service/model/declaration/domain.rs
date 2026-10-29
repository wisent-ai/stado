use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Declared domain against the domain the host can have
// ---------------------------------------------------------------------------

/// Does the registry say this host is meant to keep a graphical account alive?
///
/// A headless console says nobody answers a dialog there, not that no
/// account session exists: a Mac can log an account in with no display
/// attached. Declared graphical workloads still require that session's
/// domain, so a headless console alone does not move their units into the
/// system domain.
pub fn declared_graphical(target: &ComputeTarget) -> bool {
    target.weles.as_ref().is_some_and(|policy| policy.enabled) || target.display_stream.is_some()
}

/// Is `label` one of Stado's own units: the host unit the catalog names, or
/// a unit named after it (`com.wisent.stado-network`)? A catalog that cannot
/// be read names no unit.
pub fn is_stado_unit(label: &str) -> bool {
    match crate::deploy::service_catalog::host_unit() {
        Ok(host) => label
            .strip_prefix(host.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-')),
        Err(_unreadable_catalog) => false,
    }
}

/// Must the unit `label` run in launchd's system domain on this host?
///
/// On a Mac whose recorded console is headless (`targets[].console`,
/// measured by `stado host console record`), nobody answers the questions
/// macOS asks before a program reaches the local network, and macOS asks a
/// LaunchAgent and never a launchd daemon. Stado's own units reach other
/// hosts and their network's peers, so they are daemons there whatever else
/// the host runs. Every other unit is a daemon there unless the host declares
/// a graphical workload, whose units need the account's session. A host with
/// no recorded console, an attended one and a Linux host keep per-login
/// units.
pub fn requires_daemon_domain(target: &ComputeTarget, label: &str) -> bool {
    target.declared_headless() && (is_stado_unit(label) || !declared_graphical(target))
}

/// A unit declared in a launchd domain its host's recorded console rules out.
///
/// A unit declared as a LaunchAgent (`/Users/<user>/Library/LaunchAgents/...`)
/// on a Mac whose recorded console is headless runs where macOS asks the
/// person at the screen before it reaches the local network, and nobody is
/// there to answer; with nobody logged in at all, launchd builds no
/// `gui/<uid>` and the agent never loads. [`requires_daemon_domain`] says
/// which units belong in the system domain there.
///
/// The declaration is checkable without going anywhere: the path says the
/// domain and the target records its console. So this is a registry
/// finding, reported by `stado registry doctor` and printed under `stado
/// service list`, rather than a dialog nobody sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisdeclaredDomain {
    /// Registry target the unit is declared on.
    pub host: String,
    /// The declared service name `stado service ensure` takes.
    pub name: String,
    /// The host's own name for the unit — the launchd label.
    pub unit: String,
    /// The unit-file path the declaration carries.
    pub path: String,
    /// The domain that path places the unit in, as [`UnitDomain::as_str`]
    /// spells it.
    pub declared_domain: &'static str,
    /// The domain the unit belongs in on this host.
    pub loadable_domain: &'static str,
    /// Where the daemon spelling of this unit belongs.
    pub daemon_path: String,
    /// The recorded console's evidence, in the words an operator reads.
    pub console: String,
}

impl MisdeclaredDomain {
    /// The finding for one declared unit, or `None` when the declaration and
    /// the host agree.
    ///
    /// Registry-declared units only. A `host_recovery::managed_agents` entry
    /// is carried by that fixed program and not by the document, so it is
    /// not a registry finding and correcting the document would not move it.
    pub fn detect(target: &ComputeTarget, service: &ManagedService) -> Option<Self> {
        let unit = service.unit_id().to_string();
        if service.source != SOURCE_REGISTRY || !requires_daemon_domain(target, &unit) {
            return None;
        }
        let declared = UnitDomain::from_path(&service.path);
        if !declared.is_per_login() {
            return None;
        }
        let console = match target.console() {
            Ok(Some(console)) => console.evidence(),
            Ok(None) | Err(_) => return None,
        };
        Some(Self {
            host: target.name.clone(),
            name: service.name.clone(),
            daemon_path: format!("{LAUNCHD_DAEMONS}/{unit}.plist"),
            unit,
            path: service.path.clone(),
            declared_domain: declared.as_str(),
            loadable_domain: DOMAIN_SYSTEM,
            console,
        })
    }

    /// The command that moves this unit into the system domain:
    /// `service ensure --as-daemon` installs the daemon, stops the agent,
    /// reads the daemon until it answers, and only then removes the agent,
    /// putting it back when the daemon does not answer.
    pub fn install_command(&self) -> String {
        format!(
            "stado service ensure {} --host {} --as-daemon --reason \"nobody answers macOS at \
             this host's screen\"",
            self.name, self.host
        )
    }

    /// The one sentence both surfaces print: the unit, the domain it
    /// declares, the console that rules it out, and the command that closes
    /// the gap.
    pub fn sentence(&self) -> String {
        format!(
            "{} is declared in launchd's {} domain ({}), and {} records a headless console ({}), \
             so macOS's questions to that agent are never answered; it belongs in the {} domain \
             at {}: {}",
            self.unit,
            self.declared_domain,
            self.path,
            self.host,
            self.console,
            self.loadable_domain,
            self.daemon_path,
            self.install_command()
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "name": self.name,
            "unit": self.unit,
            "path": self.path,
            "declared_domain": self.declared_domain,
            "loadable_domain": self.loadable_domain,
            "daemon_path": self.daemon_path,
            "console": self.console,
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
