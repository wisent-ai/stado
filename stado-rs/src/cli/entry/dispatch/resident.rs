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

use std::path::Path;
use std::process::Stdio;

use crate::deploy::service::{
    self, KIND_LAUNCHD, KIND_SYSTEMD, LAUNCHD_DAEMONS, USER_LAUNCHD_AGENTS,
};

/// Why the unit is rendered again, as `service ensure --reason` records it.
const REASON: &str =
    "the resident unit's argument vector is refused by the executable it starts";
/// Where the converge's own output is kept, beside this account's Stado logs.
const LOG: &str = "~/.stado/logs/resident-converge.log";
/// What the registry is read for, opening the sentence of a refused read.
const PURPOSE: &str = "rendering the resident unit again";
/// The converge last started, by pid and the kernel's birth second for it:
/// init's next start of the unit starts no second one while the process born
/// then still holds that pid, and a pid reused by another process stops
/// nothing.
const RUNNING: &str = "~/.stado/logs/resident-converge.json";

/// What [`RUNNING`] holds.
#[derive(serde::Deserialize)]
struct Converge {
    pid: i32,
    born: u64,
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

/// The converge started for the resident unit whose argument vector is this
/// process's own, said as one sentence; `None` when no resident unit of this
/// machine starts this exact argv.
async fn converge() -> Result<Option<String>, String> {
    let product = crate::deploy::service_catalog::host_process()?.name;
    let label = crate::deploy::service_catalog::host_unit()?;
    let home = crate::config_file::expand_tilde("~");
    let started: Vec<String> = std::env::args_os()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    for path in service::resident_unit_paths(&home, &label) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(format!("cannot read {}: {cause}", path.display())),
        };
        let launchd = path.starts_with(home.join(USER_LAUNCHD_AGENTS))
            || path.starts_with(LAUNCHD_DAEMONS);
        let kind = if launchd { KIND_LAUNCHD } else { KIND_SYSTEMD };
        let unit = service::parse_local_unit_file(&text, kind)
            .map_err(|cause| format!("cannot parse {}: {cause}", path.display()))?;
        if unit.arguments != started {
            continue;
        }
        let program = std::env::current_exe()
            .map_err(|cause| format!("this executable cannot be named: {cause}"))?;
        let host = self_host().await?;
        let log = crate::config_file::expand_tilde(LOG);
        if let Some(directory) = log.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|cause| format!("cannot create {}: {cause}", directory.display()))?;
        }
        let running = crate::config_file::expand_tilde(RUNNING);
        if let Some(pid) = still_running(&running)? {
            return Ok(Some(format!(
                "{label} ({}) starts {} with an argument vector this build refuses; \
                 the converge started earlier (pid {pid}) is still rendering it, and \
                 writes its answer to {}",
                path.display(),
                program.display(),
                log.display()
            )));
        }
        let arguments: Vec<String> = [
            "service",
            "ensure",
            product.as_str(),
            "--host",
            host.as_str(),
            "--reason",
            REASON,
            "--json",
        ]
        .iter()
        .map(|word| word.to_string())
        .collect();
        let child = crate::cli::fleet::ingress::spawn_detached(
            &program,
            &arguments,
            Stdio::null(),
            &log,
        )
        .map_err(|cause| cause.to_string())?;
        record(&running, child.id())?;
        return Ok(Some(format!(
            "{label} ({}) starts {} with an argument vector this build refuses; \
             `{} service ensure {product} --host {host}` (pid {}) renders it again from this \
             executable's own catalog and restarts it, and writes its answer to {}",
            path.display(),
            program.display(),
            program.display(),
            child.id(),
            log.display()
        )));
    }
    Ok(None)
}

/// The pid of the converge `running` records, when the process holding that
/// pid now is the one born at the recorded second.
fn still_running(running: &Path) -> Result<Option<i32>, String> {
    let recorded = match std::fs::read_to_string(running) {
        Ok(recorded) => recorded,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(format!("cannot read {}: {cause}", running.display())),
    };
    // The record only keeps a second converge from starting; one that cannot
    // be read is said and the converge starts.
    let converge: Converge = match serde_json::from_str(&recorded) {
        Ok(converge) => converge,
        Err(cause) => {
            eprintln!(
                "stado: {} records no converge ({cause}), so none is taken as running",
                running.display()
            );
            return Ok(None);
        }
    };
    let born = crate::wait::open::birth_seconds(converge.pid)?;
    Ok(Some(converge.pid).filter(|_| born == Some(converge.born)))
}

/// Record the converge `child` in `running` with its kernel birth second;
/// a converge that has already ended leaves nothing to record.
fn record(running: &Path, child: u32) -> Result<(), String> {
    let pid = i32::try_from(child)
        .map_err(|cause| format!("the converge's pid {child} is no process id: {cause}"))?;
    let Some(born) = crate::wait::open::birth_seconds(pid)? else {
        return Ok(());
    };
    let text = serde_json::json!({ "pid": pid, "born": born }).to_string();
    std::fs::write(running, text)
        .map_err(|cause| format!("cannot record the converge in {}: {cause}", running.display()))
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
        None => Err(format!("host {hostname} is not in this machine's registry copy")),
    }
}
