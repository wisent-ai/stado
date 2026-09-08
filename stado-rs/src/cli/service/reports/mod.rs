//! The reads and diagnostics of `stado service`: the fleet-wide lists, one
//! unit's status and the evidence behind a `failed` one, the host probes, and
//! the single-unit views.
//!
//! [`FailureEvidence`] and [`render_status`] live here rather than beside
//! `status`, because two commands render the same table: `list` walks the
//! whole declared set from the beacons, `status` one unit's rows.

use super::*;

pub(crate) mod list;
pub(crate) mod probe;
pub(crate) mod status;
pub(crate) mod view;

/// Why one `failed` unit died, gathered best-effort from the host itself.
/// Every read can fail — the host may be the thing that is broken — so a
/// failed read degrades to a note, never to a failed `status`.
struct FailureEvidence {
    host: String,
    unit: String,
    /// launchd's last exit status for the label, when `launchctl list`
    /// carried it.
    last_exit: Option<String>,
    /// Where the stderr tail came from, or the reason there is none.
    error_origin: Option<String>,
    error_lines: Vec<String>,
    /// Why gathering failed, when it did.
    note: Option<String>,
}

impl FailureEvidence {
    fn push_note(&mut self, note: String) {
        match &mut self.note {
            Some(existing) => {
                existing.push_str("; ");
                existing.push_str(&note);
            }
            None => self.note = Some(note),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "last_exit": self.last_exit,
            "error_origin": self.error_origin,
            "error_lines": self.error_lines,
            "note": self.note,
        })
    }
}

fn render_status(
    rows: &[ServiceStatus],
    json: bool,
    failures: &[FailureEvidence],
) -> Result<(), CmdError> {
    // Read once for the whole table. The record is a file on this machine and
    // the answer is the same for every row in one rendering.
    let seen = observations::load();
    if json {
        let payload: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut entry = row.to_json();
                // Carried in the machine-readable form too, because the
                // consumers of `--json` are the dashboards and gates that
                // acted on a twelve-day-old `active` without ever being able
                // to see how old it was.
                let fact = observations::service_fact(&row.service.name, &row.service.host);
                entry["observed"] = json!(observations::describe_in(&seen, &fact));
                if let Some(failure) = failures.iter().find(|failure| {
                    failure.host == row.service.host && failure.unit == row.service.unit_id()
                }) {
                    entry["failure"] = failure.to_json();
                }
                entry
            })
            .collect();
        return print_json(&Value::Array(payload));
    }
    // The domain column appears only when at least one row is a system
    // LaunchDaemon: that is the one domain the approved channel cannot
    // bootstrap, and a fleet of user-domain units should not pay for a
    // column that says nothing.
    let show_domain = rows
        .iter()
        .any(|row| UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap());
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let fact = observations::service_fact(&row.service.name, &row.service.host);
            let mut cells = vec![
                row.service.host.clone(),
                row.service.name.clone(),
                row.service.unit_id().to_string(),
                row.service.source.clone(),
                row.state.clone(),
                dash(&row.reported_at),
                observations::describe_in(&seen, &fact),
                dash(&row.detail),
            ];
            if show_domain {
                cells.insert(3, dash(UnitDomain::from_path(&row.service.path).as_str()));
            }
            cells
        })
        .collect();
    let mut headers = vec![
        "HOST",
        "SERVICE",
        "UNIT",
        "SOURCE",
        "STATE",
        "REPORTED_AT",
        "OBSERVED",
        "DETAIL",
    ];
    if show_domain {
        headers.insert(3, "DOMAIN");
    }
    table::print(&headers, &cells);
    // A unit declared in a domain its host cannot have, named where the
    // operator is already reading the table it is missing from. This is a
    // fact about the document, so it prints for every row that carries it
    // whatever the beacon said — including the `missing` rows, where the
    // declaration is the reason the beacon reports nothing.
    for row in rows {
        if let Some(misdeclared) = &row.misdeclared_domain {
            println!("declaration: {}", misdeclared.sentence());
        }
    }
    for failure in failures {
        let exit = match &failure.last_exit {
            Some(exit) => format!("last launchd exit {exit}"),
            None => "last launchd exit unknown".to_string(),
        };
        println!("failure: {} {}: {}", failure.host, failure.unit, exit);
        // A failed system LaunchDaemon has exactly one repair over the
        // approved channel, and it has conditions; say which, here, where the
        // operator is reading why.
        if rows.iter().any(|row| {
            row.service.host == failure.host
                && row.service.unit_id() == failure.unit.as_str()
                && UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap()
        }) {
            println!(
                "  unit: system LaunchDaemon — `service restart` can only end its process and let \
                 launchd's KeepAlive replace it; loading it takes sudo on the host"
            );
        }
        if let Some(error_origin) = &failure.error_origin {
            println!("  stderr: {error_origin}");
        }
        for line in &failure.error_lines {
            println!("  {line}");
        }
        if let Some(note) = &failure.note {
            println!("  note: {note}");
        }
    }
    Ok(())
}
