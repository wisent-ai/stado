use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

/// `stado host gates HOST [--json]` — why this host is claiming nothing, in
/// one payload.
///
/// The exit status follows `claiming`, the way `host ping`'s follows its
/// combined verdict, so `stado space reclaim mini --apply --reason … && stado
/// host gates mini` is a usable sentence and a blocked host cannot be
/// mistaken for a healthy one by a script that only reads status codes.
///
/// The Mac mini sat at roughly 2 GiB free against a 55 GiB policy, its agent
/// published `disk_pressure_unresolved` every tick, it claimed nothing for
/// hours, every release build queued behind it — and no command in this CLI
/// said any of it. This is that sentence.
pub async fn gates(host: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let gates = crate::deploy::host_gates::read_host_gates(host, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let report = Value::Object(crate::deploy::host_gates::to_report(&gates));
    if json {
        print_json(&report);
        return claiming_outcome(&gates);
    }
    println!("host:     {}", gates.host);
    println!("claiming: {}", if gates.claiming { "yes" } else { "no" });
    if gates.claiming {
        println!("blockers: none");
    } else {
        // The agent's own words, unabridged: whatever is printed here has to
        // be greppable in the code that published it.
        println!("blockers: {}", gates.blockers.join(", "));
    }
    println!(
        "disk:     {} free, low watermark {}, target {}, policy {}",
        gigabytes(gates.free_gb),
        gigabytes(gates.low_watermark_gb.map(|gb| gb as f64)),
        gigabytes(gates.target_free_gb.map(|gb| gb as f64)),
        gates.policy_mode.as_deref().unwrap_or("none declared"),
    );
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
            let admission = match gates.accepting_jobs {
                Some(true) => "accepting jobs",
                Some(false) => "busy or gated",
                None => "admission unstated",
            };
            let cpu = gates
                .available_cpu_cores
                .zip(gates.total_cpu_cores)
                .map_or_else(
                    || "-/-".to_string(),
                    |(free, total)| format!("{free}/{total}"),
                );
            let ram = gates.free_ram_gb.zip(gates.total_ram_gb).map_or_else(
                || "-/-".to_string(),
                |(free, total)| format!("{free:.1}/{total:.1}"),
            );
            let vram = gates.free_vram_gb.zip(gates.total_vram_gb).map_or_else(
                || "-/-".to_string(),
                |(free, total)| format!("{free}/{total}"),
            );
            println!(
                "capacity: {admission}, {} running job(s), CPU {cpu} cores available/total, \
                 RAM {ram} GiB free/total, VRAM {vram} GiB free/total; published {} ({published})",
                gates.running_jobs.unwrap_or_default(),
                gates.age_seconds.map_or_else(
                    || "at an unknown time".to_string(),
                    |age| format!(
                        "{} ago",
                        crate::cli::registry::human_age(chrono::TimeDelta::seconds(age))
                    )
                ),
            );
            if !gates.available_accelerators.is_empty() {
                println!(
                    "accelerators: {}",
                    serde_json::to_string(&gates.available_accelerators)
                        .unwrap_or_else(|_| "{}".to_string())
                );
            }
        }
        None => {
            println!("capacity: nothing published for this host, so the scheduler cannot see it")
        }
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
                .map(|job| {
                    let id = &job.job_id[..8.min(job.job_id.len())];
                    match job.age_seconds {
                        Some(age) => format!(
                            "{id} ({} in queue)",
                            crate::cli::registry::human_age(chrono::TimeDelta::seconds(age))
                        ),
                        None => id.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
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
                gates
                    .local_snapshots
                    .map_or_else(|| "-".to_string(), |count| count.to_string()),
                gates.host,
            );
            continue;
        }
        println!("note:     {note}");
    }
    claiming_outcome(&gates)
}

/// GiB with one decimal, or a dash for a number this host did not answer with.
fn gigabytes(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |gb| format!("{gb} GiB"))
}

/// What the two backend names mean when they do not agree, read off the
/// blocker [`crate::deploy::host_gates`] already decided.
///
/// Keyed off the blocker and never re-classified here: a second classifier of
/// storage backends in the CLI would eventually disagree with the one in the
/// reader about one host, and the operator would believe whichever line they
/// read first.
fn store_clause(blockers: &[String]) -> &'static str {
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_DEVICE_ONLY)
    {
        return " — a store only that host can address, so nothing its agent publishes ever \
                reaches this fleet";
    }
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_UNKNOWN)
    {
        return " — a backend this build has no adapter for, so how far that agent's writes \
                carry cannot be decided here";
    }
    ""
}

/// A host that is not claiming is a failed verdict, not a failed command: the
/// read succeeded either way, and the message names the blockers rather than
/// repeating that something is wrong.
fn claiming_outcome(gates: &crate::deploy::host_gates::HostGates) -> Result<(), CmdError> {
    if gates.claiming {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} is claiming nothing: {}",
        gates.host,
        gates.blockers.join(", ")
    )))
}
