//! A declared unit its init system keeps restarting is said where the
//! operator's work is tracked, not only in the beacon's `failed` word.
//!
//! Each health pass hands every declared unit it read, with its failing
//! verdict (`LabelState::failing`), to [`observe`]. A failing unit with no
//! Oko defect raised for its crash loop raises one; the loop is kept in
//! `~/.stado/failing-units.json` under the unit's label with what raising it
//! answered, so a crash loop is one defect, not one per pass. A unit read as
//! not failing ends its loop, and a later failure is a new one. A raise Oko
//! refused or could not take is kept with that cause and tried again on the
//! next pass. A unit the pass could not read is not handed over, and its
//! loop stays as it was.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One unit's crash loop as the health passes saw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Loop {
    /// When a pass first read the unit failing in this loop.
    first_seen: String,
    /// The newest failing verdict a pass read.
    failing: String,
    /// Whether Oko recorded the defect of this loop.
    reported: bool,
    /// What raising it answered: the defect or Oko's refusal.
    report: String,
}

fn path() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".stado")
                .join("failing-units.json")
        })
        .ok_or_else(|| {
            "HOME is not set, so the crash loops of this host's units have no file to be kept in"
                .to_string()
        })
}

fn read(path: &PathBuf) -> Result<BTreeMap<String, Loop>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            format!(
                "{} does not hold the crash loops of this host's units ({error})",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

fn raise(service: &str, label: &str, host: &str, seen: &Loop) -> Result<String, String> {
    super::record_defect(&super::Defect {
        surface: &format!("service {service} ({label}) on {host}"),
        title: &format!("{service} restarts without serving on {host}: {}", seen.failing),
        found_in: &format!("{label} on {host}"),
        impact: "the service is down while its init system restarts it, and everything that reaches it fails",
        evidence: &format!(
            "a stado health pass on {host} read {label} failing since {}: {}",
            seen.first_seen, seen.failing
        ),
        repair: "read the unit's log (stado service logs), repair the release or the configuration it refuses so that it starts, and deliver the repair to the host",
    })
}

/// Keep this pass's verdicts and raise the defect of every crash loop none
/// was raised for yet. `declared` is every unit the registry declares on
/// this host; a loop of a unit no longer declared is forgotten, as Stado no
/// longer runs it. `units` maps each unit read this pass, by label, to its
/// service name and its failing verdict. `Ok` with what Oko answered for
/// each unit now failing, by label, for the beacon to carry; `Err` with why
/// the loops could not be read or kept.
pub(crate) fn observe(
    declared: &std::collections::BTreeSet<String>,
    units: &BTreeMap<String, (String, Option<String>)>,
) -> Result<BTreeMap<String, String>, String> {
    let path = path()?;
    let kept = read(&path)?;
    let mut loops = kept.clone();
    loops.retain(|label, _| declared.contains(label));
    // The host is named only in a defect, so it is read once and only when
    // a pass raises one.
    let mut host: Option<String> = None;
    let mut reports = BTreeMap::new();
    for (label, (service, failing)) in units {
        let Some(failing) = failing else {
            loops.remove(label);
            continue;
        };
        let now = chrono::Utc::now().to_rfc3339();
        let seen = loops.entry(label.clone()).or_insert_with(|| Loop {
            first_seen: now,
            failing: failing.clone(),
            reported: false,
            report: String::new(),
        });
        seen.failing = failing.clone();
        if !seen.reported {
            let host = host.get_or_insert_with(super::host);
            let raised = raise(service, label, host, seen);
            seen.reported = raised.is_ok();
            seen.report = match raised {
                Ok(report) | Err(report) => report,
            };
        }
        reports.insert(label.clone(), seen.report.clone());
    }
    // A pass that changed nothing writes nothing: most passes read every
    // unit serving and keep an empty register.
    if loops == kept {
        return Ok(reports);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{} cannot be created: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&loops)
        .map_err(|error| format!("the crash loops cannot be encoded: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    Ok(reports)
}
