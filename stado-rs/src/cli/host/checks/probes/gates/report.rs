//! The same answer written for a person: every read, the verdict, the
//! blockers in the agent own words, and the capacity behind them.

use crate::cli::reporting::{field, NOT_REPORTED};
use crate::deploy::host_gates::HostGates;

use super::outcome::{gigabytes, store_clause};

/// What this command read off the host, or the words saying it was not
/// observed: a reading it could not take is never printed as a value.
const NOT_OBSERVED: &str = "not observed";

fn observed(value: Option<impl std::fmt::Display>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => NOT_OBSERVED.to_string(),
    }
}

/// A figure the host's capacity publication carries, or the words saying it
/// did not report it.
fn reported(value: Option<impl std::fmt::Display>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => NOT_REPORTED.to_string(),
    }
}

/// Print what this host answered, line by line.
pub(super) fn print_report(gates: &HostGates) {
    println!("host:     {}", gates.host);
    println!(
        "diagnostic: {}",
        if gates.complete {
            "complete"
        } else {
            "incomplete"
        }
    );
    for read in &gates.observations {
        println!(
            "read:     {} {:?}; {} ms; source {}",
            read.operation, read.state, read.elapsed_ms, read.source
        );
        if let Some(detail) = &read.detail {
            println!("detail:   {detail}");
        }
    }
    println!(
        "claiming: {}",
        if !gates.complete {
            "unknown"
        } else if gates.claiming {
            "yes"
        } else {
            "no"
        }
    );
    if gates.claiming {
        println!("blockers: none");
    } else {
        // The agent's own words, unabridged: whatever is printed here has to
        // be greppable in the code that published it.
        println!("blockers: {}", gates.blockers.join(", "));
    }
    println!(
        "presence: {}",
        crate::deploy::host_gates::Presence::of(gates).line()
    );
    println!(
        "disk:     {} free, {} used, {} until the {}% disk-full threshold",
        gigabytes(gates.free_gb),
        observed(gates.used_percent.map(|used| format!("{used}%"))),
        gigabytes(gates.headroom_gb),
        crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT,
    );
    // Beside the disk line and never inside it: the disk and memory refuse
    // work for different reasons and are repaired differently.
    println!("memory:   {}", observed(gates.memory.line()));
    // Which of the two sources answered, on its own line: a memory reading
    // taken by this command off a silent host and one the host published
    // itself are different facts, and an operator acting on the first has to
    // know the agent is not talking.
    println!("memory evidence: {}", observed(gates.memory.source));
    println!("pressure evidence: {}", observed(gates.pressure_source));
    // Why this command could not measure the disk itself, beside the
    // evidence it did or did not have: the thing to repair on the host.
    if let Some(cause) = gates.local_disk_unread.as_deref() {
        println!("local disk reading: {cause}");
    }
    // Both stores on one line, ahead of the capacity line the first one
    // explains: an agent bound to a device-local store publishes capacity into
    // a store nothing here reads, and an operator who cannot see the two
    // backend names side by side reads `capacity_publication_stale` and goes
    // looking at the agent's uptime instead of at what its unit exports.
    match gates.agent_store_backend.as_deref() {
        Some(backend) => println!(
            "store:    agent writes to {backend}, this control plane reads {}{}",
            gates.fleet_store_backend,
            store_clause(&gates.blockers),
        ),
        None => println!(
            "store:    this host did not answer with a storage backend, so where its agent \
             publishes cannot be shown; this control plane reads {}",
            gates.fleet_store_backend
        ),
    }
    match gates.published_at.as_deref() {
        Some(published) => {
            let admission = match (
                gates.accepting_jobs,
                gates.admission_reason.as_deref(),
                gates.admission_detail.as_deref(),
            ) {
                (Some(true), _, _) => "accepting jobs".to_string(),
                (Some(false), Some(reason), Some(detail)) => {
                    format!("not accepting jobs: {reason} ({detail})")
                }
                (Some(false), Some(reason), None) => format!("not accepting jobs: {reason}"),
                (Some(false), None, Some(detail)) => format!("not accepting jobs: {detail}"),
                (Some(false), None, None) => "busy or gated".to_string(),
                (None, _, _) => "admission unstated".to_string(),
            };
            let cpu = format!(
                "{}/{}",
                reported(gates.available_cpu_cores),
                reported(gates.total_cpu_cores)
            );
            let ram = format!(
                "{}/{}",
                reported(gates.free_ram_gb),
                reported(gates.total_ram_gb)
            );
            let vram = format!(
                "{}/{}",
                reported(gates.free_vram_gb),
                reported(gates.total_vram_gb)
            );
            let age = match gates.age_seconds {
                Some(age) => format!(
                    "{} ago",
                    crate::cli::registry::human_age(chrono::TimeDelta::seconds(age))
                ),
                None => "at a time it did not report".to_string(),
            };
            println!(
                "capacity: {admission}, {} running job(s), CPU {cpu} cores available/total, \
                 RAM {ram} GiB free/total, VRAM {vram} GiB free/total; \
                 published {age} ({published})",
                reported(gates.running_jobs),
            );
            if let Some(accelerators) = gates
                .available_accelerators
                .as_ref()
                .filter(|accelerators| !accelerators.is_empty())
            {
                match serde_json::to_string(accelerators) {
                    Ok(accelerators) => println!("accelerators: {accelerators}"),
                    Err(error) => println!("accelerators: cannot be written out: {error}"),
                }
            }
            if let Some(line) = gates
                .published_diagnostics
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(crate::providers::local::accelerators::accelerator_holders_line)
            {
                println!("accelerator: {line}");
            }
            if let Some(held) = gates.running_workloads.filter(|held| held.is_positive()) {
                let number = |key: &str| match &gates.reserved {
                    Some(reserved) => field(&reserved[key]),
                    None => NOT_REPORTED.to_string(),
                };
                println!(
                    "reserved: {held} placed workload(s) hold {} core(s), {} GiB RAM, {} GiB VRAM; the capacity line is net of them",
                    number("cpu_cores"),
                    number("ram_gb"),
                    number("vram_gb")
                );
                match &gates.reservations {
                    Some(reservations) => {
                        for reservation in reservations {
                            println!(
                                "          {} ({}) held by {} since {}: {} core(s), {} GiB",
                                field(&reservation["kind"]),
                                field(&reservation["product"]),
                                field(&reservation["holder"]),
                                field(&reservation["acquired_at"]),
                                field(&reservation["cpu_cores"]),
                                field(&reservation["ram_gb"])
                            );
                        }
                    }
                    None => println!("          the host could not read its reservations"),
                }
            }
        }
        None => match &gates.publication_attempt {
            Some(attempt)
                if attempt.outcome == crate::queue::capacity::attempt::Outcome::Refused =>
            {
                println!(
                    "capacity: nothing published for this host, so the scheduler cannot see it; \
                     its worker cannot reach the fleet store: its {} at {} was refused{} (Stado \
                     {}): {}",
                    attempt.step,
                    attempt.at,
                    attempt
                        .refused_since
                        .map_or(String::new(), |since| format!(", refused since {since}")),
                    attempt.stado_version,
                    attempt
                        .error
                        .as_deref()
                        .unwrap_or("the store gave no words")
                )
            }
            Some(attempt) => println!(
                "capacity: nothing published for this host here, though its worker's last \
                 capacity write was accepted by the store its own configuration names ({}) at {} \
                 (Stado {}); compare that store with the one this control plane reads ({}), and \
                 the record's time with now",
                gates.agent_store_backend.as_deref().unwrap_or("not read"),
                attempt.at,
                attempt.stado_version,
                gates.fleet_store_backend
            ),
            None => println!(
                "capacity: nothing published for this host, so the scheduler cannot see it; the \
                 account this command reaches it as keeps no record of a publication attempt \
                 (~/.stado/{}): its worker has not run under a Stado that keeps one (upgrade it, \
                 or start its worker), or it runs with another home than this account's (a unit \
                 whose environment sets no HOME; set HOME in the unit)",
                crate::queue::capacity::attempt::RECORD
            ),
        },
    }
    // The consequence beside the cause: what this host's refusal is starving,
    // oldest first, so "blocked" has a size and an age.
    if !gates.waiting_jobs.is_empty() {
        println!(
            "waiting:  {} pinned job(s) this host is not taking: {}",
            gates.waiting_jobs.len(),
            gates
                .waiting_jobs
                .iter()
                .map(|job| match job.age_seconds {
                    Some(age) => format!(
                        "{} ({} in queue)",
                        job.job_id,
                        crate::cli::registry::human_age(chrono::TimeDelta::seconds(age))
                    ),
                    None => job.job_id.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        // The host's own reason, per job: `claiming: yes, blockers: none`
        // above a week-old waiting build is only true for jobs the host can
        // take, and without this line it read as a healthy, merely busy host.
        for job in &gates.waiting_jobs {
            if let Some(reason) = &job.declined {
                println!("declined: {}: {reason}", job.job_id);
            }
        }
    }
    // Printed after the verdict and never as part of it: a note is a thing the
    // operator has to know before they conclude the numbers do not add up, and
    // `stado space reclaim` is about to tell them it freed less than the deficit.
    for note in &gates.notes {
        if note == crate::deploy::host_gates::LOCAL_SNAPSHOTS_UNRECLAIMABLE {
            println!(
                "note:     {note} — {} local APFS snapshot(s), which macOS reports no size \
                 for. `stado space reclaim {} --stage local_apfs_snapshots` may thin local \
                 Time Machine snapshots to the declared watermark; `com.apple.os.update-*` \
                 snapshots remain OS recovery state, so no Stado command deletes them",
                observed(gates.local_snapshots),
                gates.host,
            );
            continue;
        }
        if note == crate::deploy::host_gates::DISK_ATTACHED_UNMOUNTED {
            println!(
                "note:     {note} — {}; the fleet writes under the agent's home and cannot \
                 use a disk nothing has mounted. `stado space report {}` lists every volume",
                gates.unmounted_disks.join(", "),
                gates.host,
            );
            continue;
        }
        println!("note:     {note}");
    }
}
