//! The resident unit rendered again by the executable that refuses its
//! argument vector.
//!
//! An installation migrates the resident unit before it replaces the
//! executable, but a unit an older Stado wrote, or one edited by hand, can
//! still carry an option the installed executable no longer takes. Init then
//! starts that executable, it refuses its argv and exits, and init starts it
//! again without end: the only process that could reconcile the unit is the
//! one that keeps dying. That process is also the one whose compiled catalog
//! says what the unit must run, so when the argv clap refused is exactly the
//! argument vector of this machine's resident unit, it starts its own
//! `service ensure` for the unit before it exits, and says so on stderr,
//! which init keeps in the unit's log.
//!
//! The ensure runs as the leader of its own process group, so the unit's
//! restart (launchd ends the job's group) does not end it half way, and it
//! writes its answer to [`LOG`]. A refusal of any other argv, a help or
//! version request and a process that is not the resident unit are left to
//! clap untouched.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::deploy::service::{
    self, KIND_LAUNCHD, KIND_SYSTEMD, LAUNCHD_DAEMONS, USER_LAUNCHD_AGENTS,
};

/// Why the unit is rendered again, as `service ensure --reason` records it.
const REASON: &str = "the resident unit's argument vector is refused by the executable it starts";
/// Where the converge's own output is kept, beside this account's Stado logs.
const LOG: &str = "~/.stado/logs/resident-converge.log";
/// What the registry is read for, opening the sentence of a refused read.
const PURPOSE: &str = "rendering the resident unit again";
/// The converge last started, by pid and the kernel's birth second for it,
/// and the argument vector of the process it was started for: init's next
/// start of the unit starts no second one while the process born then
/// still holds that pid, a pid reused by another process stops nothing, and
/// a process still running the line a converge already ran for starts none.
const RUNNING: &str = "~/.stado/logs/resident-converge.json";

/// What [`RUNNING`] holds. `born` is `null` for a converge that had already
/// ended when it was recorded, which no process holds any more. A record
/// written before the line was kept names none, so no process is taken as
/// one a converge already ran for.
#[derive(serde::Deserialize)]
struct Converge {
    pid: i32,
    born: Option<u64>,
    #[serde(default)]
    started_for: Vec<String>,
}

/// A converge of the resident unit, as [`start`] left it.
pub(crate) enum Converging {
    /// The converge recorded earlier still runs, by pid.
    Running(i32),
    /// This call started one, by pid.
    Started(u32),
}

/// What [`start`] did, with the product it ensures, the program that runs
/// the ensure and the log it writes its answer to.
pub(crate) struct Started {
    pub state: Converging,
    pub product: String,
    pub program: PathBuf,
    pub log: PathBuf,
}

/// Start `<this executable> service ensure <host product> --host <host>
/// --reason <reason>` as the leader of its own process group, writing its
/// answer to [`LOG`], and record it for `started_for`, the argument vector
/// of the process it renders again; when the converge recorded earlier
/// still runs, start none and say which.
pub(crate) fn start(host: &str, reason: &str, started_for: &[String]) -> Result<Started, String> {
    let product = crate::deploy::service_catalog::host_process()?.name;
    let program = std::env::current_exe()
        .map_err(|cause| format!("this executable cannot be named: {cause}"))?;
    let log = crate::config_file::expand_tilde(LOG);
    if let Some(directory) = log.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|cause| format!("cannot create {}: {cause}", directory.display()))?;
    }
    let running = crate::config_file::expand_tilde(RUNNING);
    if let Some(pid) = still_running(&running)? {
        let state = Converging::Running(pid);
        return Ok(Started {
            state,
            product,
            program,
            log,
        });
    }
    let arguments: Vec<String> = [
        "service",
        "ensure",
        product.as_str(),
        "--host",
        host,
        "--reason",
        reason,
        "--json",
    ]
    .iter()
    .map(|word| word.to_string())
    .collect();
    let child =
        crate::cli::fleet::ingress::spawn_detached(&program, &arguments, Stdio::null(), &log)
            .map_err(|cause| cause.to_string())?;
    record(&running, child.id(), started_for)?;
    let state = Converging::Started(child.id());
    Ok(Started {
        state,
        product,
        program,
        log,
    })
}

/// Whether the converge recorded last was started for `line`, the argument
/// vector of a process still running it: that converge did not change what
/// the unit starts, and starting another for the same line renders the same
/// unit again. A record that cannot be read names no line.
pub(crate) fn ran_for(line: &[String]) -> Result<bool, String> {
    let running = crate::config_file::expand_tilde(RUNNING);
    match read_record(&running)? {
        Some(converge) => Ok(converge.started_for == line),
        None => Ok(false),
    }
}

/// When `refused` is clap's refusal of this machine's resident unit's own
/// argument vector, start the unit's converge through this executable and
/// say what was started; say why it could not be started otherwise. Clap
/// still answers the refusal and exits after this returns.
pub(super) async fn converge_refused(refused: &clap::Error) {
    // Help and version requests are answers on stdout, not refusals.
    if !refused.use_stderr() {
        return;
    }
    match converge().await {
        Ok(Some(started)) => eprintln!("stado: {started}"),
        Ok(None) => {}
        Err(cause) => eprintln!(
            "stado: this process may be the resident unit, but it was not rendered again: {cause}"
        ),
    }
}

/// This process's whole argument vector, program first, as init started it.
pub(crate) fn own_line() -> Vec<String> {
    std::env::args_os()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

/// The resident unit file of this machine whose argument vector is `line`,
/// or `None` when no resident unit starts exactly that line: a process
/// started by hand, or by a test under a home of its own, is no resident
/// unit, and nothing renders a unit again for it.
pub(crate) fn resident_unit_of(line: &[String]) -> Result<Option<PathBuf>, String> {
    let label = crate::deploy::service_catalog::host_unit()?;
    let home = crate::config_file::expand_tilde("~");
    for path in service::resident_unit_paths(&home, &label) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(format!("cannot read {}: {cause}", path.display())),
        };
        let launchd =
            path.starts_with(home.join(USER_LAUNCHD_AGENTS)) || path.starts_with(LAUNCHD_DAEMONS);
        let kind = if launchd { KIND_LAUNCHD } else { KIND_SYSTEMD };
        let unit = service::parse_local_unit_file(&text, kind)
            .map_err(|cause| format!("cannot parse {}: {cause}", path.display()))?;
        if unit.arguments == line {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The converge started for the resident unit whose argument vector is this
/// process's own, said as one sentence; `None` when no resident unit of this
/// machine starts this exact argv.
async fn converge() -> Result<Option<String>, String> {
    let label = crate::deploy::service_catalog::host_unit()?;
    let started = own_line();
    let Some(path) = resident_unit_of(&started)? else {
        return Ok(None);
    };
    let host = self_host().await?;
    let Started {
        state,
        product,
        program,
        log,
    } = start(&host, REASON, &started)?;
    Ok(Some(match state {
        Converging::Running(pid) => format!(
            "{label} ({}) starts {} with an argument vector this build refuses; \
             the converge started earlier (pid {pid}) is still rendering it, and \
             writes its answer to {}",
            path.display(),
            program.display(),
            log.display()
        ),
        Converging::Started(pid) => format!(
            "{label} ({}) starts {} with an argument vector this build refuses; \
             `{} service ensure {product} --host {host}` (pid {pid}) renders it again from \
             this executable's own catalog and restarts it, and writes its answer to {}",
            path.display(),
            program.display(),
            program.display(),
            log.display()
        ),
    }))
}

/// The converge `running` records, or `None` when there is no record. A
/// record that cannot be read only keeps a converge from starting, so it is
/// said and read as none.
fn read_record(running: &Path) -> Result<Option<Converge>, String> {
    let recorded = match std::fs::read_to_string(running) {
        Ok(recorded) => recorded,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(format!("cannot read {}: {cause}", running.display())),
    };
    match serde_json::from_str(&recorded) {
        Ok(converge) => Ok(Some(converge)),
        Err(cause) => {
            eprintln!(
                "stado: {} records no converge ({cause}), so none is taken as recorded",
                running.display()
            );
            Ok(None)
        }
    }
}

/// The pid of the converge `running` records, when the process holding that
/// pid now is the one born at the recorded second; a converge recorded as
/// already ended runs no more.
fn still_running(running: &Path) -> Result<Option<i32>, String> {
    let Some(converge) = read_record(running)? else {
        return Ok(None);
    };
    let Some(recorded) = converge.born else {
        return Ok(None);
    };
    let born = crate::wait::open::birth_seconds(converge.pid)?;
    Ok((born == Some(recorded)).then_some(converge.pid))
}

/// Record the converge `child` in `running` with its kernel birth second and
/// the line it was `started_for`. A converge that has already ended has no
/// birth to record and is recorded with `born` null, keeping its line, so a
/// converge that failed at once is not started again for the same line.
fn record(running: &Path, child: u32, started_for: &[String]) -> Result<(), String> {
    let pid = i32::try_from(child)
        .map_err(|cause| format!("the converge's pid {child} is no process id: {cause}"))?;
    let born = crate::wait::open::birth_seconds(pid)?;
    let text = serde_json::json!({ "pid": pid, "born": born, "started_for": started_for });
    std::fs::write(running, text.to_string()).map_err(|cause| {
        format!(
            "cannot record the converge in {}: {cause}",
            running.display()
        )
    })
}

/// The registry target this machine is, read from what this machine already
/// holds: the resident unit is down, so the authority it serves cannot be
/// asked.
async fn self_host() -> Result<String, String> {
    let hostname = crate::providers::vast::system_hostname();
    let (registry, notice) = crate::targets::registry_without_authority(PURPOSE)
        .await
        .map_err(|cause| format!("this machine's registry record cannot be read: {cause}"))?;
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    let found = registry
        .lookup_self(&hostname)
        .map_err(|cause| format!("host {hostname} cannot be looked up: {cause}"))?;
    match found {
        Some(target) => Ok(target.name.clone()),
        None => Err(format!(
            "host {hostname} is not in this machine's registry copy"
        )),
    }
}
