//! What the init system says of one unit the takeover weighs: whether it is
//! loaded, the pid it names as its main process, and the environment its
//! process starts with.

use std::path::Path;

use super::served_root;

/// What the init system reports for one loaded unit: launchd's `print`, or
/// systemd's `MainPID` and `Environment` properties.
pub(super) struct UnitState {
    text: String,
    systemd: bool,
}

impl UnitState {
    /// The pid the init system names as the unit's main process; `None` when
    /// it names none: no such row, or systemd's `MainPID=0` for a unit with no
    /// process. A row whose value is not a pid is refused naming it.
    pub(super) fn main_pid(&self) -> Result<Option<u32>, String> {
        let prefix = if self.systemd { "MainPID=" } else { "pid = " };
        let Some(value) = self
            .text
            .lines()
            .find_map(|line| line.trim().strip_prefix(prefix))
        else {
            return Ok(None);
        };
        let pid = value.trim().parse::<u32>().map_err(|error| {
            format!("the init system names main pid {value:?}, which is not a pid: {error}")
        })?;
        Ok(std::num::NonZeroU32::new(pid).map(std::num::NonZeroU32::get))
    }

    /// One environment variable the unit starts its process with.
    fn variable(&self, key: &str) -> Option<String> {
        if self.systemd {
            let environment = self
                .text
                .lines()
                .find_map(|line| line.strip_prefix("Environment="))?;
            return environment
                .split_whitespace()
                .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_string);
        }
        self.text.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(" => ")?;
            (name == key).then(|| value.trim().to_string())
        })
    }

    /// Why this unit is not proven to serve `served_root`, the local root this
    /// process is about to serve, comparing canonical paths; `None` when it
    /// serves exactly that root. A process with no local root is unproven,
    /// never the same. The unit's root is resolved the way its own process
    /// resolves it ([`served_root::resolve`]), so a unit that declares nothing
    /// and reads the same config as this process is the same root.
    pub(super) fn other_root(&self, served_root: Option<&str>) -> Option<String> {
        let Some(ours) = served_root.map(|root| served_root::canonical(Path::new(root))) else {
            return Some(
                "this process serves no local store, so the route it takes over \
                         cannot be proven the same"
                    .to_string(),
            );
        };
        let home = match crate::config_file::home() {
            Ok(home) => home,
            Err(cause) => return Some(format!("its served root cannot be resolved: {cause}")),
        };
        let theirs = match served_root::resolve(&|key| self.variable(key), &home) {
            Ok(theirs) => theirs,
            Err(error) => return Some(format!("its served root cannot be resolved: {error}")),
        };
        (!served_root::serves_local_root(&theirs.backend) || theirs.root != ours).then(|| {
            format!(
                "it serves backend {:?} root {:?} by {}, this process serves {ours}",
                theirs.backend, theirs.root, theirs.source
            )
        })
    }
}

/// The loaded state of `unit`: launchd's system domain, then this account's
/// GUI domain; systemd's system manager, then this account's user manager.
/// `Ok(None)` when none of them holds it. An init system that cannot be read
/// (a `sudo -n` the host refuses, a manager that does not answer) is an error
/// naming the command and how it ended: "not read" is not "not loaded", and a
/// predecessor whose root was never read must not be retired as the same root.
pub(super) fn unit_state(unit: &str) -> Result<Option<UnitState>, String> {
    let run = |program: &str, args: &[&str]| -> Result<String, String> {
        let command = format!("{program} {}", args.join(" "));
        let output = crate::wait::output(std::process::Command::new(program).args(args))
            .map_err(|error| format!("{command} could not run: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "{command} ended with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    if cfg!(target_os = "macos") {
        let label = match unit.strip_suffix(".service") {
            Some(label) => label,
            None => unit,
        };
        // Presence is read from each domain's listing and only a held service
        // is printed; only a held system service's print needs `sudo`.
        if crate::deploy::service::launchd_domain_holds("system", label)? {
            let service = format!("system/{label}");
            let text = run(
                "/usr/bin/sudo",
                &["-n", "/bin/launchctl", "print", &service],
            )?;
            return Ok(Some(UnitState {
                text,
                systemd: false,
            }));
        }
        let gui = format!("gui/{}", nix::unistd::getuid());
        if crate::deploy::service::launchd_domain_holds(&gui, label)? {
            let service = format!("{gui}/{label}");
            let text = run("/bin/launchctl", &["print", &service])?;
            return Ok(Some(UnitState {
                text,
                systemd: false,
            }));
        }
        return Ok(None);
    }
    let service = if unit.ends_with(".service") {
        unit.to_string()
    } else {
        format!("{unit}.service")
    };
    // `systemctl show` answers for an absent unit too, with a `LoadState`
    // other than `loaded`, so a call that fails is a manager not read.
    let properties = "--property=LoadState,MainPID,Environment";
    let loaded = |text: String| {
        text.lines()
            .any(|line| line == "LoadState=loaded")
            .then_some(text)
    };
    let system = run(
        "/usr/bin/sudo",
        &["-n", "/bin/systemctl", "show", properties, &service],
    )?;
    let text = match loaded(system) {
        Some(text) => Some(text),
        None => loaded(run(
            "/bin/systemctl",
            &["--user", "show", properties, &service],
        )?),
    };
    Ok(text.map(|text| UnitState {
        text,
        systemd: true,
    }))
}
