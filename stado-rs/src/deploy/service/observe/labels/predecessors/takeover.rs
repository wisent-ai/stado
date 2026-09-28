//! The host Stado process takes the API listener over from the units that
//! held it before, when it starts under its own unit to serve the API.
//!
//! The units whose role is the API listener (`role_units` with `--api`) hold
//! the very port the replacement binds: a unit that was renamed runs the same
//! program under the old label. The replacement could never come up beside
//! it, and nothing else would retire it, because the reconciler that retires
//! predecessors runs inside that old process. So once the API's store is
//! prepared and before it binds, each such unit this host still loads is
//! booted out and its autostart withdrawn, provided it serves the same root.
//! `stado serve --api` and `stado dashboard` both run it. Nothing else
//! retires these units: a flag in a live argument vector proves neither a
//! bound listener nor the same root. Each retirement is recorded on the host
//! as `taken_over`, and that record is what keeps ensure and the reconciler
//! from repairing the old unit afterwards, see [`taken_over`].

use crate::deploy::service::*;

use super::{record, retirement, PredecessorRetirement};

/// The handoff record state of an API listener unit this host's Stado
/// process retired at API start.
const TAKEN_OVER: &str = "taken_over";

/// The unit the init system started this process under: launchd names the
/// job in `XPC_SERVICE_NAME`, systemd in the process's own cgroup path.
/// `None` for a process nothing started as a unit, such as a shell command.
fn own_unit() -> Option<String> {
    if let Ok(label) = std::env::var("XPC_SERVICE_NAME") {
        return Some(label);
    }
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    cgroup.lines().find_map(|line| {
        let unit = line.rsplit('/').next()?;
        unit.strip_suffix(".service").map(str::to_string)
    })
}

/// Retire, on this host, every API listener predecessor of the host Stado
/// process, when this process is that unit's own main process and is
/// about to serve the object API from `served_root`. Both the launchd label
/// and the cgroup are inherited by children, so the name alone proves
/// nothing: the init system must also bind the unit to this pid. A process
/// started any other way, a child of any unit, or an old unit restarting
/// retires nothing.
async fn take_over_retired(
    served_root: Option<&str>,
    runner: &Runner,
) -> Result<Vec<PredecessorRetirement>, DeployError> {
    let Some(unit) = own_unit() else {
        return Ok(Vec::new());
    };
    let entry = crate::deploy::service_catalog::host_process().map_err(DeployError)?;
    let predecessors = crate::deploy::service_catalog::api_predecessors(&entry);
    if entry.unit.as_deref() != Some(unit.as_str()) || predecessors.is_empty() {
        return Ok(Vec::new());
    }
    let owner = unit_state(&unit).as_ref().and_then(UnitState::main_pid);
    if owner != Some(std::process::id()) {
        eprintln!(
            "[stado] {unit} names pid {owner:?} as its main process, not this pid {}; \
             its predecessors are left to that process",
            std::process::id()
        );
        return Ok(Vec::new());
    }
    let target = this_host()?;
    let mut retirements = Vec::with_capacity(predecessors.len());
    for retired in predecessors {
        // A predecessor serving another storage root is an authority change,
        // which only `stado host storage-root-reconcile` may make.
        if let Some(refusal) = unit_state(retired).and_then(|state| state.other_root(served_root)) {
            retirements.push(PredecessorRetirement {
                unit: retired.to_string(),
                state: "failed".to_string(),
                detail: format!(
                    "{refusal}; moving the object store is `stado host storage-root-reconcile`, \
                     not a takeover"
                ),
            });
            continue;
        }
        let retired = retirement(&target, retired, runner).await;
        if retired.state != "failed" {
            let pid = std::process::id().to_string();
            if let Err(error) =
                record::write_record(&target, &retired.unit, TAKEN_OVER, &[], &pid, runner).await
            {
                retirements.push(PredecessorRetirement {
                    detail: format!(
                        "{}; its takeover could not be recorded, so the reconciler would \
                         repair it: {error}",
                        retired.detail
                    ),
                    state: "failed".to_string(),
                    unit: retired.unit,
                });
                continue;
            }
        }
        retirements.push(retired);
    }
    Ok(retirements)
}

/// The takeover this host's Stado process recorded for API listener unit
/// `unit`: the pid that retired it and since when. `None` when none was
/// recorded, or the record cannot be read, so the unit is still repaired.
pub(super) async fn taken_over(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Option<String> {
    let record = record::read_record(target, unit, runner).await.ok()??;
    (record.state == TAKEN_OVER).then(|| {
        format!(
            "pid {} retired it at API start on the same storage root (epoch {})",
            record.artefact, record.since
        )
    })
}

/// The local backend word a predecessor must name to serve the same root.
const LOCAL_BACKEND: &str = "local";

/// What the init system reports for one loaded unit: launchd's `print`, or
/// systemd's `MainPID` and `Environment` properties.
struct UnitState {
    text: String,
    systemd: bool,
}

impl UnitState {
    fn main_pid(&self) -> Option<u32> {
        let prefix = if self.systemd { "MainPID=" } else { "pid = " };
        self.text.lines().find_map(|line| {
            line.trim()
                .strip_prefix(prefix)
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|pid| *pid != u32::MIN)
        })
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
    /// serves exactly that root. A process with no local root, or a unit that
    /// does not declare its backend and root, is unproven, never the same.
    fn other_root(&self, served_root: Option<&str>) -> Option<String> {
        let canonical = |value: &str| {
            std::fs::canonicalize(value)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| value.to_string())
        };
        let Some(ours) = served_root.map(canonical) else {
            return Some(
                "this process serves no local store, so the route it takes over \
                         cannot be proven the same"
                    .to_string(),
            );
        };
        let backend = self.variable("WC_STORAGE_BACKEND");
        let theirs = self
            .variable("WC_LOCAL_STORAGE_PATH")
            .map(|path| canonical(&path));
        (backend.as_deref() != Some(LOCAL_BACKEND) || theirs.as_deref() != Some(ours.as_str()))
            .then(|| {
                format!(
                    "it runs with WC_STORAGE_BACKEND={backend:?} WC_LOCAL_STORAGE_PATH={theirs:?}, \
                     this process serves {ours}"
                )
            })
    }
}

/// The loaded state of `unit`: launchd's system domain, then this account's
/// GUI domain; systemd's system manager, then this account's user manager.
/// `None` when none of them holds it.
fn unit_state(unit: &str) -> Option<UnitState> {
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    if cfg!(target_os = "macos") {
        let label = unit.strip_suffix(".service").unwrap_or(unit);
        let system = format!("system/{label}");
        let user = format!("gui/{}/{label}", nix::unistd::getuid());
        return run("/usr/bin/sudo", &["-n", "/bin/launchctl", "print", &system])
            .or_else(|| run("/bin/launchctl", &["print", &user]))
            .map(|text| UnitState {
                text,
                systemd: false,
            });
    }
    let service = if unit.ends_with(".service") {
        unit.to_string()
    } else {
        format!("{unit}.service")
    };
    let properties = "--property=LoadState,MainPID,Environment";
    run(
        "/usr/bin/sudo",
        &["-n", "/bin/systemctl", "show", properties, &service],
    )
    .filter(|text| text.lines().any(|line| line == "LoadState=loaded"))
    .or_else(|| {
        run("/bin/systemctl", &["--user", "show", properties, &service])
            .filter(|text| text.lines().any(|line| line == "LoadState=loaded"))
    })
    .map(|text| UnitState {
        text,
        systemd: true,
    })
}

/// [`take_over_retired`] with the production runner, run by an API listener
/// once its storage is prepared and before it binds, with the local root that
/// storage serves (`None` for a store with no local root). Each outcome goes
/// to stderr, where the unit's log keeps it, and a unit that stays loaded is
/// the error the process exits with, because it holds what the process is
/// about to bind.
pub async fn take_over_on_start(served_root: Option<&str>) -> Result<(), String> {
    let retirements = take_over_retired(served_root, &crate::deploy::production_runner())
        .await
        .map_err(|error| format!("could not retire this process's predecessors: {error}"))?;
    let mut failed = Vec::new();
    for retirement in retirements {
        eprintln!(
            "[stado] predecessor {}: {} ({})",
            retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "refusing to start beside predecessors that could not be retired: {}",
        failed.join("; ")
    ))
}

/// This machine as a target of the host channel, which then runs every
/// script locally: the name and the one hostname the channel matches on.
fn this_host() -> Result<ComputeTarget, DeployError> {
    let hostname = crate::providers::vast::system_hostname();
    if hostname.is_empty() {
        return Err(DeployError(
            "this host's name could not be read, so its retired units cannot be addressed"
                .to_string(),
        ));
    }
    serde_json::from_value(serde_json::json!({
        "name": hostname,
        "kind": "local",
        "hostnames": [hostname],
    }))
    .map_err(|error| {
        DeployError(format!(
            "this host could not be described as a target: {error}"
        ))
    })
}
