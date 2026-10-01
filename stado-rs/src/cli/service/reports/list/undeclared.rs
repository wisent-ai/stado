//! `service list --undeclared`.

use super::*;

/// `service list --undeclared` — launchd jobs a host has loaded that the
/// registry does not declare, fleet-wide.
///
/// The third question, and the one that had no answer. [`list`] walks the
/// document and asks the host about each entry. [`list_unowned`] walks the
/// processes and asks launchd who owns them. A unit launchd has LOADED and the
/// document has never heard of is in neither set, so nothing in this binary
/// could name one.
///
/// A host can run several queue agents at once in that blind spot: the one
/// the registry declares, one from `stado bootstrap --local`'s label
/// convention, and one from an older installer. All of them publish capacity
/// for the same consumer id, so whichever writes last decides what the host
/// answers — and an agent running a stale binary can refuse pinned jobs for
/// days while every report in this group says the declared agent is fine.
///
/// An empty answer means the hosts were asked and had nothing, because a host
/// that will not answer is named on stderr and makes the command fail.
///
/// It also means the whole host was asked. Enumerating only labels under
/// `com.wisent.` would make an empty answer a fact about that prefix, read as
/// a fact about the machine: a stray agent under another prefix, holding the
/// pid that rewrites the janitor's state file every interval, would go
/// unreported. Every row is enumerated and classified; the prefix chooses
/// the sentence, never the population.
pub(crate) async fn list_undeclared(json: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let runner = production_runner();
    let mut found: Vec<service::UndeclaredUnit> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        match service::undeclared_units(target, &runner).await {
            Ok(units) => found.extend(units),
            Err(exc) => failures.push(format!("{}: {exc}", target.name)),
        }
    }
    // A label the fleet never named but the host ties to the fleet anyway is
    // the more interesting class and used to be the invisible one, so it reads
    // first. This is the order rows are printed in and nothing else.
    found.sort_by(|left, right| {
        let rank = |unit: &service::UndeclaredUnit| match unit.classification() {
            "outside-fleet-prefix" => 0,
            "undeclared" => 1,
            _ => 2,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.label.cmp(&right.label))
    });
    if json {
        // Every row, every class. The JSON answer is the complete one, so
        // nothing below can be the only place a label exists.
        let payload: Vec<Value> = found.iter().map(service::UndeclaredUnit::to_json).collect();
        print_json(&json!({"undeclared": payload}))?;
    } else {
        // The table prints the jobs this fleet put on the host and cannot
        // account for. `unaffiliated` rows are counted below instead: on a
        // Mac they are most of the loaded labels, all of them the platform's
        // own, and printing them beside the real findings is the same
        // disservice a prefix filter would do by another route. They are read,
        // classified and counted, and `--json` carries every one of them.
        let actionable: Vec<&service::UndeclaredUnit> =
            found.iter().filter(|unit| !unit.accounted_for()).collect();
        let cells: Vec<Vec<String>> = actionable
            .iter()
            .map(|unit| {
                vec![
                    unit.host.clone(),
                    unit.classification().to_string(),
                    unit.label.clone(),
                    dash(&unit.pid),
                    unit.status.clone(),
                    // What the process IS running, and only then what its file
                    // declares. Reading only the declaration is how a job could
                    // be seen and not identified: a label named like a Stado
                    // agent can be running `python3.12 -m stado.cli agent`, a
                    // program no release of this binary can ever change.
                    dash(if unit.running_program.is_empty() {
                        &unit.program
                    } else {
                        &unit.running_program
                    }),
                    dash(&unit.path),
                ]
            })
            .collect();
        table::print(
            &[
                "HOST",
                "CLASS",
                "LABEL",
                "PID",
                "LAST_EXIT",
                "RUNS",
                "UNIT_FILE",
            ],
            &cells,
        );
        // The census, so an empty table and a table whose interesting rows are
        // outnumbered both read honestly — and so that the widening is
        // auditable: these numbers are the proof the host was asked about every
        // label rather than about one prefix.
        let count = |wanted: &str| {
            found
                .iter()
                .filter(|unit| unit.classification() == wanted)
                .count()
        };
        println!(
            "{} loaded label(s) the registry does not declare: {} outside the fleet prefix but \
             tied to it by unit file or program, {} under the prefix, {} unaffiliated with this \
             fleet and not listed above (`--json` carries every row)",
            found.len(),
            count("outside-fleet-prefix"),
            count("undeclared"),
            count("unaffiliated"),
        );
    }
    fail_if_any(&failures, "scan for undeclared units")
}
