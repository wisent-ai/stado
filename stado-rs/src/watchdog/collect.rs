//! Diagnostics collection: the diagnostic command set, one fault-isolated
//! command result, each volume as the disk-full rule reads it, and the
//! assembled payload.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::runner::{CommandRunner, RunOutcome};
use crate::models::isoformat_utc;

/// The payload's shape. Version 2 carries whole command output, volumes in
/// bytes with the disk-full rule's verdict, and the fleet's capacity read
/// through Stado rather than a cloud CLI.
const SCHEMA: &str = "wisent-box-diagnostics-v2";

/// `platform.node()`: the kernel's hostname, read where every Stado reader
/// reads it.
pub(crate) fn hostname() -> String {
    crate::providers::vast::system_hostname()
}

/// The host's own diagnostic commands: its service managers, process table,
/// accelerators, volumes and memory.
fn commands() -> Vec<(&'static str, Vec<String>)> {
    let argv = |parts: &[&str]| parts.iter().map(|part| part.to_string()).collect();
    vec![
        (
            "systemctl-agent",
            argv(&[
                "systemctl",
                "status",
                "wisent-agent.service",
                "--no-pager",
                "-l",
            ]),
        ),
        (
            "systemctl-health",
            argv(&[
                "systemctl",
                "status",
                "wisent-host-health.timer",
                "--no-pager",
                "-l",
            ]),
        ),
        (
            "journal-agent",
            argv(&[
                "journalctl",
                "-u",
                "wisent-agent.service",
                "-n",
                "240",
                "--no-pager",
            ]),
        ),
        (
            "journal-health",
            argv(&[
                "journalctl",
                "-u",
                "wisent-host-health.service",
                "-n",
                "120",
                "--no-pager",
            ]),
        ),
        (
            "ps",
            argv(&[
                "ps",
                "-eo",
                "pid,ppid,stat,pcpu,pmem,comm,args",
                "--sort=-%cpu",
            ]),
        ),
        ("nvidia-smi", argv(&["nvidia-smi"])),
        ("df", argv(&["df", "-h"])),
        ("memory", argv(&["free", "-h"])),
    ]
}

/// The command that reads the fleet's published capacity as this Stado reads
/// it: `stado capacity list --json`, run as the binary that is collecting, so
/// no `PATH` decides which Stado answers.
const CAPACITY_LIST: &str = "capacity-list";

/// One fault-isolated command result. A command this host does not have, or
/// one that could not be started, is a result saying so, never a gap.
fn run_one(name: &str, argv: &[String], runner: &dyn CommandRunner) -> Value {
    let started = std::time::Instant::now();
    let argv_json = Value::Array(argv.iter().map(|part| Value::from(part.as_str())).collect());
    match runner.run(argv) {
        Ok(RunOutcome { rc, stdout, stderr }) => json!({
            "name": name,
            "cmd": argv_json,
            "rc": rc,
            "elapsed_s": started.elapsed().as_secs_f64(),
            "stdout": stdout,
            "stderr": stderr,
        }),
        Err(error) => {
            let program = argv.first().map(String::as_str).ok_or("no program");
            let message = match (program, error.kind()) {
                (Ok(program), std::io::ErrorKind::NotFound) => {
                    format!("{program} is not installed on this host: {error}")
                }
                (Ok(program), _) => format!("{program} could not be run: {error}"),
                (Err(missing), _) => format!("{name} names {missing}: {error}"),
            };
            json!({
                "name": name,
                "cmd": argv_json,
                "rc": Value::Null,
                "elapsed_s": started.elapsed().as_secs_f64(),
                "error": message,
            })
        }
    }
}

/// The volume holding `path` as the disk-full rule reads it: its size, what
/// an unprivileged writer may still take, the percent in use, and whether the
/// rule would delete on it; or why it could not be read.
fn disk(path: &str) -> Value {
    match crate::providers::local::space_reclaim::rule::read_volume(Path::new(path)) {
        Ok(volume) => json!({
            "path": path,
            "total_bytes": volume.total_bytes,
            "free_bytes": volume.free_bytes,
            "used_percent": volume.used_percent(),
            "headroom_bytes": volume.headroom_bytes(),
            "full": volume.full(),
        }),
        Err(error) => {
            json!({"path": path, "error": format!("cannot read the volume holding {path}: {error}")})
        }
    }
}

/// Assemble the full diagnostics payload.
pub fn collect(bucket: &str, runner: &dyn CommandRunner) -> Value {
    let host = hostname();
    let home = crate::config_file::home_dir()
        .to_string_lossy()
        .into_owned();
    let mut command_results = Map::new();
    for (name, argv) in commands() {
        command_results.insert(name.to_string(), run_one(name, &argv, runner));
    }
    let capacity = match std::env::current_exe() {
        Ok(stado) => {
            let argv: Vec<String> = [
                stado.to_string_lossy().as_ref(),
                "capacity",
                "list",
                "--json",
            ]
            .iter()
            .map(|part| part.to_string())
            .collect();
            run_one(CAPACITY_LIST, &argv, runner)
        }
        Err(error) => json!({
            "name": CAPACITY_LIST,
            "rc": Value::Null,
            "error": format!("this process cannot name its own binary to read the fleet's capacity: {error}"),
        }),
    };
    command_results.insert(CAPACITY_LIST.to_string(), capacity);
    json!({
        "schema": SCHEMA,
        "reported_at": isoformat_utc(chrono::Utc::now()),
        "host": host,
        "bucket": bucket,
        "pid": std::process::id(),
        "disk": [disk("/"), disk(&home), disk("/tmp")],
        "commands": Value::Object(command_results),
    })
}
