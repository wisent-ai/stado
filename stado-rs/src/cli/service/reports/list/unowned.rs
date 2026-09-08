//! `service list --unowned`.

use super::*;

/// `service list --unowned` — product processes no unit owns, fleet-wide.
///
/// The one read in this group that cannot come off the beacons: a beacon
/// reports the units the host was told about, and an unowned process is by
/// construction in nobody's declaration. So it is one read-only ssh per
/// kind=local host, and a host that will not answer is named on stderr rather
/// than dropped — "no unowned processes" and "nobody looked" are the fold this
/// whole group refuses to make.
/// One host's per-candidate ownership verdicts: the host, then
/// `(pid, "owned"|"unowned", the ancestor pid launchd claimed)` for each.
type HostVerdicts = (String, Vec<(String, String, String)>);

pub(crate) async fn list_unowned(json: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let runner = production_runner();
    let mut found: Vec<service::UnownedProcess> = Vec::new();
    let mut accounts: Vec<String> = Vec::new();
    let mut verdicts: Vec<HostVerdicts> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        match service::unowned_processes(target, &runner).await {
            Ok(scan) => {
                accounts.push(scan.account(&target.name));
                verdicts.push((target.name.clone(), scan.judged.clone()));
                found.extend(scan.processes);
            }
            Err(exc) => failures.push(format!("{}: {exc}", target.name)),
        }
    }
    if json {
        let payload: Vec<Value> = found.iter().map(service::UnownedProcess::to_json).collect();
        let judged: Vec<Value> = verdicts
            .iter()
            .flat_map(|(host, rows)| {
                rows.iter().map(move |(pid, verdict, owner)| {
                    json!({"host": host, "pid": pid, "verdict": verdict, "claimed_by": owner})
                })
            })
            .collect();
        print_json(&json!({"unowned": payload, "searched": accounts, "judged": judged}))?;
    } else {
        let cells: Vec<Vec<String>> = found
            .iter()
            .map(|process| {
                vec![
                    process.host.clone(),
                    process.pid.clone(),
                    process.product_guess(),
                    dash(&process.started_at),
                    process.command.clone(),
                ]
            })
            .collect();
        table::print(
            &["HOST", "PID", "PRODUCT_GUESS", "STARTED_AT", "COMMAND"],
            &cells,
        );
        // Printed on every run, not only the empty one: a table with three rows
        // and a root that matched nothing is the same unread answer as an empty
        // table, one root later.
        for account in &accounts {
            println!("searched {account}");
        }
        for (host, judged) in &verdicts {
            for (pid, verdict, owner) in judged {
                println!("judged {host}: pid {pid} {verdict} (launchd claimed {owner})");
            }
        }
    }
    fail_if_any(&failures, "scan for unowned processes")
}
