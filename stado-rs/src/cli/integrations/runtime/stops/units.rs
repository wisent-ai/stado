//! A declared unit its init system keeps restarting is said to the operator,
//! not only in the beacon's `failed` word.
//!
//! Each health pass hands every declared unit it read, with its failing
//! verdict (`LabelState::failing`, which carries the exit code and the
//! supervisor's run count), to [`observe`]. The first pass that reads a unit
//! failing pages the operator through Stado's own alert channels (the
//! registry's `operator_contact`) with the unit, the verdict and the last
//! line it wrote to stderr, and records an Oko defect for the loop. The loop
//! is kept in `~/.stado/failing-units.json` under the unit's label with what
//! each answered, so a crash loop is one page and one defect, not one per
//! pass. A page no channel took, or a defect Oko refused, is tried again on
//! the next pass; an Oko that is not installed is said once and not asked
//! again for that loop, and the page goes out without it. A unit read as not
//! failing ends its loop, and a later failure is a new one. A unit the pass
//! could not read is not handed over, and its loop stays as it was.
//!
//! The transcript lake's stream restarted 89,948 times over fourteen days
//! before anyone saw it: the beacon said `failed`, and the defect hand-off
//! was refused by Oko on every pass, so nothing ever reached the operator.

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
    /// Whether Oko recorded the defect of this loop, or is not installed to.
    reported: bool,
    /// What raising it answered: the defect, Oko's refusal, or that Oko is
    /// not installed.
    report: String,
    /// Whether a channel took the operator's page for this loop.
    #[serde(default)]
    alerted: bool,
    /// What paging answered, per channel, or why no channel was paged.
    #[serde(default)]
    alert: String,
}

fn path() -> Result<PathBuf, String> {
    crate::config_file::home()
        .map(|home| home.join(".stado").join("failing-units.json"))
        .map_err(|cause| {
            format!("the crash loops of this host's units have no file to be kept in: {cause}")
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

fn raise(service: &str, label: &str, host: &str, seen: &Loop) -> Result<String, super::Unrecorded> {
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
        blocks_nothing: "no command meets it: the init system restarts the unit, and what fails is every request that reaches the service",
    })
}

/// One unit read this pass: its service name, its failing verdict, and the
/// last line it wrote to stderr when it is failing and that line was read.
pub(crate) struct UnitVerdict {
    pub service: String,
    pub failing: Option<String>,
    pub last_error: Option<String>,
}

/// The operator's page for a loop, and what each channel answered. `Ok` when
/// a channel took it.
async fn page(
    service: &str,
    label: &str,
    host: &str,
    unit: &UnitVerdict,
    seen: &Loop,
) -> Result<String, String> {
    let last = match &unit.last_error {
        Some(line) => format!("its last error: {line}"),
        None => "its last error line could not be read; `stado service status` shows the log"
            .to_string(),
    };
    let message = format!(
        "{service} ({label}) on {host} restarts without serving since {}: {}; {last}. Repair the \
         release or the configuration it refuses so that it starts, and deliver the repair to \
         the host.",
        seen.first_seen, seen.failing
    );
    let subject = format!("{service} restarts without serving on {host}");
    let deliveries =
        crate::monitor::alerts::send_alert(crate::config::alerts_topic(), &message, &subject).await;
    let said = deliveries
        .iter()
        .map(|delivery| format!("{}: {}", delivery.channel, delivery.detail))
        .collect::<Vec<_>>()
        .join("; ");
    if deliveries.iter().any(|delivery| delivery.delivered) {
        Ok(format!("paged: {said}"))
    } else if said.is_empty() {
        Err(
            "no alert channel resolves, so the operator was not paged; `stado alerts \
             channels` names why"
                .to_string(),
        )
    } else {
        Err(format!("no alert channel took the page: {said}"))
    }
}

/// Keep this pass's verdicts, page the operator and raise the defect of
/// every crash loop that has not had them yet. `declared` is every unit the
/// registry declares on this host; a loop of a unit no longer declared is
/// forgotten, as Stado no longer runs it. `units` maps each unit read this
/// pass, by label, to its verdict. `Ok` with what paging and Oko answered for
/// each unit now failing, by label, for the beacon to carry; `Err` with why
/// the loops could not be read or kept.
pub(crate) async fn observe(
    declared: &std::collections::BTreeSet<String>,
    units: &BTreeMap<String, UnitVerdict>,
) -> Result<BTreeMap<String, String>, String> {
    let path = path()?;
    let kept = read(&path)?;
    let mut loops = kept.clone();
    loops.retain(|label, _| declared.contains(label));
    // The host is named only in a page or a defect, so it is read once and
    // only when a pass raises one.
    let mut host: Option<String> = None;
    let mut reports = BTreeMap::new();
    for (label, unit) in units {
        let Some(failing) = &unit.failing else {
            loops.remove(label);
            continue;
        };
        let now = chrono::Utc::now().to_rfc3339();
        let seen = loops.entry(label.clone()).or_insert_with(|| Loop {
            first_seen: now,
            failing: failing.clone(),
            reported: false,
            report: String::new(),
            alerted: false,
            alert: String::new(),
        });
        seen.failing = failing.clone();
        if !seen.alerted {
            let host = host.get_or_insert_with(super::host).clone();
            let paged = page(&unit.service, label, &host, unit, seen).await;
            seen.alerted = paged.is_ok();
            seen.alert = match paged {
                Ok(answer) | Err(answer) => answer,
            };
        }
        if !seen.reported {
            let host = host.get_or_insert_with(super::host).clone();
            match raise(&unit.service, label, &host, seen) {
                Ok(report) => {
                    seen.reported = true;
                    seen.report = report;
                }
                // An Oko that is not installed is not asked again for this
                // loop: the page above is what reaches the operator.
                Err(super::Unrecorded::OkoAbsent(sentence)) => {
                    seen.reported = true;
                    seen.report = sentence;
                }
                Err(super::Unrecorded::Refused(sentence)) => seen.report = sentence,
            }
        }
        reports.insert(
            label.clone(),
            format!("operator page: {}; Oko defect: {}", seen.alert, seen.report),
        );
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
