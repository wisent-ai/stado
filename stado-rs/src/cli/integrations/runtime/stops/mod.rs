//! A Stado release that stops on every start is a failed install, and is
//! said where the operator's work is tracked instead of being restarted
//! unseen by the init system.
//!
//! Every time `stado serve` fails — a component stopped, or startup could
//! not finish — the stop is appended to `~/.stado/serve-stops.jsonl` with
//! this binary's version and revision and the failure. A stop of a build
//! that already stopped before on this host is that release failing again:
//! the first time that is seen the failure is recorded as an Oko defect
//! against Stado with the stops and their causes, and the entry is marked
//! reported, so a crash loop is one defect, not one per restart. No count or
//! window is Stado's: a second stop of the same build is the measurement.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::binary::build_identity::{SOURCE_REVISION, VERSION};

/// One stop of `stado serve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stop {
    version: String,
    revision: String,
    at: String,
    failure: String,
    /// Whether this stop raised the Oko defect for its build.
    #[serde(default)]
    reported: bool,
    /// What raising it answered: the defect or Oko's refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report: Option<String>,
}

fn path() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".stado").join("serve-stops.jsonl"))
        .ok_or_else(|| {
            "HOME is not set, so stado serve's stops have no file to be kept in".to_string()
        })
}

fn read(path: &PathBuf) -> Result<Vec<Stop>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "{} holds a line that is not a stop ({error}): {line}",
                    path.display()
                )
            })
        })
        .collect()
}

/// The host this process serves, for the defect's surface.
fn host() -> String {
    match crate::wait::output(&mut std::process::Command::new("hostname")) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => format!("a host whose hostname ended {}", output.status),
        Err(error) => format!("a host whose hostname could not be read ({error})"),
    }
}

/// Record the defect of a build that stopped again: `oko defects record`
/// with the build, the host, every stop of the build and its failure. It is
/// found in the service on this host, not in `stado@<revision>`: Oko accepts
/// a revision only where its checkout is, and a host Stado serves on holds
/// no checkout; the revision is in the evidence.
fn raise(stops: &[&Stop], latest: &Stop) -> String {
    let host = host();
    let mut evidence = vec![format!(
        "stado {} (revision {}) on {host}, kept in ~/.stado/serve-stops.jsonl:",
        latest.version, latest.revision
    )];
    evidence.extend(
        stops
            .iter()
            .chain(std::iter::once(&latest))
            .map(|stop| format!("{}: {}", stop.at, stop.failure)),
    );
    let first_line = latest.failure.lines().next().map(str::trim);
    let title = format!(
        "Stado {} stops on every start on {host}: {}",
        latest.version,
        first_line.into_iter().collect::<String>()
    );
    let mut command = std::process::Command::new("oko");
    command.args([
        "defects",
        "record",
        "--product",
        "stado",
        "--surface",
        &format!("stado serve on {host}"),
        "--title",
        &title,
        "--found-in",
        &format!("stado serve {} on {host}", latest.version),
        "--impact",
        "every service this stado serve hosts on the host (resolver adapters, the object API, the queue worker) is down while its init system restarts it, and nothing else reports it",
        "--evidence",
        &evidence.join("\n"),
        "--repair",
        "repair the fault in this release, or the configuration it refuses, so that stado serve starts; deliver the repair to the host",
    ]);
    match crate::wait::output(&mut command) {
        Ok(output) if output.status.success() => {
            format!(
                "recorded: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            )
        }
        Ok(output) => format!(
            "oko defects record ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("oko defects record could not run: {error}"),
    }
}

/// Keep this stop, and raise the defect when this build already stopped
/// before and none was raised for it yet. What could not be kept or raised
/// is said on stderr: the service is failing either way, and the cause of
/// its failure is not replaced by the cause of the report's.
pub(crate) fn record(failure: &dyn std::fmt::Display) {
    if let Err(error) = keep(failure) {
        eprintln!("stado serve's stop could not be kept: {error}");
    }
}

fn keep(failure: &dyn std::fmt::Display) -> Result<(), String> {
    let path = path()?;
    let earlier = read(&path)?;
    let mut stop = Stop {
        version: VERSION.to_string(),
        revision: SOURCE_REVISION.to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        failure: failure.to_string(),
        reported: false,
        report: None,
    };
    let same: Vec<&Stop> = earlier
        .iter()
        .filter(|before| before.version == stop.version && before.revision == stop.revision)
        .collect();
    if !same.is_empty() && !same.iter().any(|before| before.reported) {
        let report = raise(&same, &stop);
        eprintln!(
            "stado serve {} stopped again on this host; {report}",
            stop.version
        );
        stop.reported = true;
        stop.report = Some(report);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{} cannot be created: {error}", parent.display()))?;
    }
    let line = serde_json::to_string(&stop)
        .map_err(|error| format!("the stop cannot be encoded: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("{} cannot be opened: {error}", path.display()))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|error| format!("{} cannot be appended to: {error}", path.display()))
}

/// The stops of the newest build that stopped on this host, as `stado
/// resolver status` shows them: its version and revision, how many times it
/// stopped, the first and latest stop with the latest failure, and what
/// raising its Oko defect answered (`null` while it stopped once). `null`
/// when `stado serve` never stopped here; `{unreadable}` with the cause when
/// the kept stops cannot be read.
pub(crate) fn newest_build() -> serde_json::Value {
    let stops = match path().and_then(|path| read(&path)) {
        Ok(stops) => stops,
        Err(error) => return serde_json::json!({ "unreadable": error }),
    };
    let Some(latest) = stops.last() else {
        return serde_json::Value::Null;
    };
    let same: Vec<&Stop> = stops
        .iter()
        .filter(|stop| stop.version == latest.version && stop.revision == latest.revision)
        .collect();
    serde_json::json!({
        "version": latest.version,
        "revision": latest.revision,
        "stops": same.len(),
        "first_at": same.first().map(|stop| stop.at.clone()),
        "latest_at": latest.at,
        "latest_failure": latest.failure,
        "report": same.iter().find_map(|stop| stop.report.clone()),
    })
}
