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
//! The same measurement decides the rollback ([`failed_install`]): a stop
//! that repeats an earlier stop of the same build, or a stop on this
//! release's own rules against the host's configuration, proves the install
//! failed, and the service reinstalls the release before it. The register
//! itself (`stado_product::serve_stops`) is shared with the source install,
//! which holds a revision whose stops raised a defect.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use stado_product::serve_stops::{read, Stop};

use crate::binary::build_identity::{SOURCE_REVISION, VERSION};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

fn path() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| stado_product::serve_stops::path(&PathBuf::from(home)))
        .ok_or_else(|| {
            "HOME is not set, so stado serve's stops have no file to be kept in".to_string()
        })
}

/// The stops of this build kept on this host so far.
fn earlier_stops() -> Result<Vec<Stop>, String> {
    Ok(read(&path()?)?
        .into_iter()
        .filter(|before| before.version == VERSION && before.revision == SOURCE_REVISION)
        .collect())
}

/// Why `failure`, the stop of this `stado serve`, proves this release's
/// install failed on this host, as the cause its rollback records: a
/// configuration failure is this release's rules refusing the configuration
/// the release before it served, on its first stop; any other failure once
/// this build already stopped here before. `None` when the stop proves
/// nothing yet, or when the kept stops cannot be read (said on stderr: a
/// loop nobody can measure is not rolled back on a guess).
pub(crate) fn failed_install(failure: &CmdError) -> Option<String> {
    if failure.failure == Some(FailureCode::Config) {
        return Some(format!(
            "this release's rules refuse the host's configuration: {failure}"
        ));
    }
    let earlier = match earlier_stops() {
        Ok(earlier) => earlier,
        Err(error) => {
            eprintln!("stado serve's earlier stops cannot be read, so this stop is not measured against them: {error}");
            return None;
        }
    };
    let first = earlier.first()?;
    Some(format!(
        "stado serve {VERSION} stopped {} time(s) on this host before this one, the first at {} with: {}; this one with: {failure}",
        earlier.len(),
        first.at,
        first.failure
    ))
}

pub(crate) mod units;

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

/// What `oko defects record` is told about a failure Stado saw on a host.
struct Defect<'a> {
    surface: &'a str,
    title: &'a str,
    found_in: &'a str,
    impact: &'a str,
    evidence: &'a str,
    repair: &'a str,
}

/// Record `defect` against Stado through `oko defects record`. `Ok` with
/// what Oko printed when it recorded the defect, `Err` with why it did not.
fn record_defect(defect: &Defect<'_>) -> Result<String, String> {
    let mut command = std::process::Command::new("oko");
    command.args([
        "defects",
        "record",
        "--product",
        "stado",
        "--surface",
        defect.surface,
        "--title",
        defect.title,
        "--found-in",
        defect.found_in,
        "--impact",
        defect.impact,
        "--evidence",
        defect.evidence,
        "--repair",
        defect.repair,
    ]);
    match crate::wait::output(&mut command) {
        Ok(output) if output.status.success() => Ok(format!(
            "recorded: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )),
        Ok(output) => Err(format!(
            "oko defects record ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(format!("oko defects record could not run: {error}")),
    }
}

/// Record the defect of a build that stopped again: `oko defects record`
/// with the build, the host, every stop of the build and its failure. It is
/// found in the service on this host, not in `stado@<revision>`: Oko accepts
/// a revision only where its checkout is, and a host Stado serves on holds
/// no checkout; the revision is in the evidence. `Ok` with what Oko printed
/// when it recorded the defect, `Err` with why it did not.
fn raise(stops: &[&Stop], latest: &Stop) -> Result<String, String> {
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
    record_defect(&Defect {
        surface: &format!("stado serve on {host}"),
        title: &title,
        found_in: &format!("stado serve {} on {host}", latest.version),
        impact: "every service this stado serve hosts on the host (resolver adapters, the object API, the queue worker) is down while its init system restarts it, and nothing else reports it",
        evidence: &evidence.join("\n"),
        repair: "repair the fault in this release, or the configuration it refuses, so that stado serve starts; deliver the repair to the host",
    })
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
    let earlier = earlier_stops()?;
    let mut stop = Stop {
        version: VERSION.to_string(),
        revision: SOURCE_REVISION.to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        failure: failure.to_string(),
        reported: false,
        report: None,
    };
    let same: Vec<&Stop> = earlier.iter().collect();
    // A stop of a build that stopped before raises its defect, until Oko has
    // recorded one: a stop whose report Oko refused or could not take is kept
    // with that cause, and the next stop tries again.
    if !same.is_empty() && !same.iter().any(|before| before.reported) {
        let raised = raise(&same, &stop);
        stop.reported = raised.is_ok();
        let report = match raised {
            Ok(report) | Err(report) => report,
        };
        eprintln!(
            "stado serve {} stopped again on this host; {report}",
            stop.version
        );
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
/// raising its Oko defect answered (`null` while it stopped once), and
/// whether its revision is held from source installs (`held`: a stop of it
/// raised the defect, as `stado_product::serve_stops::held` reads it). `null`
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
        // The defect Oko recorded; while none is, why the newest attempt failed.
        "report": same
            .iter()
            .find(|stop| stop.reported)
            .or_else(|| same.iter().rev().find(|stop| stop.report.is_some()))
            .and_then(|stop| stop.report.clone()),
        "held": stops
            .iter()
            .any(|stop| stop.revision == latest.revision && stop.reported),
    })
}
