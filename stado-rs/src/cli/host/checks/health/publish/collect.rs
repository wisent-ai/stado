//! Collecting THIS host's health beacon: what the registry declares here,
//! what the init system holds under each of those identities, and how much
//! room the volume the fleet writes to has left.
//!
//! An init-system read refusal and a confirmed absent unit are different
//! facts. Keep the refusal in the beacon so every CLI and Desktop reader
//! describes what was observed rather than inferring inactivity.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::deploy::service::{
    STATE_ACTIVE, STATE_FAILED, STATE_INACTIVE, STATE_UNKNOWN, STATE_UNREADABLE,
};
use crate::deploy::service_label_print::{inspect_label_carrying, LabelState};

#[path = "acting.rs"]
mod acting;

use acting::{answering_entry, roles_and_strays, Strays};

/// Read one declared identity's state the way an operator would read it, and
/// keep the reason when the answer is not a state.
fn unit_entry(state: &LabelState) -> Value {
    if let Some(system) = &state.unsupported {
        return json!({"state": STATE_UNKNOWN, "detail": format!("this host runs {system}")});
    }
    if state.loaded() {
        // A unit is failed by the same verdict `service unit show` and the
        // delivery restart print: loaded, no live process, last run nonzero.
        // The supervisor's own account of it goes in `detail`, which every
        // reader of the beacon shows, so the crash loop is named and not only
        // the word `failed`. A job found while another domain refused its
        // read is real and incomplete: that is said too.
        let failing = state.failing();
        let mut entry = Map::new();
        entry.insert(
            "state".to_string(),
            Value::String(
                if failing.is_some() {
                    STATE_FAILED
                } else {
                    STATE_ACTIVE
                }
                .to_string(),
            ),
        );
        if let Some(domain) = &state.domain {
            entry.insert("domain".to_string(), Value::String(domain.clone()));
        }
        if let Some(pid) = &state.pid {
            entry.insert("pid".to_string(), Value::String(pid.clone()));
        }
        let details: Vec<String> = failing
            .into_iter()
            .chain(state.read_failure_detail())
            .collect();
        if !details.is_empty() {
            entry.insert("detail".to_string(), Value::String(details.join("; ")));
        }
        return Value::Object(entry);
    }
    match state.read_failure_detail() {
        // Nothing answered, and at least one domain would not be read. The
        // one thing this must never say is `inactive`.
        Some(detail) => json!({"state": STATE_UNREADABLE, "detail": detail}),
        None => json!({"state": STATE_INACTIVE}),
    }
}

/// `stado host beacon collect [--publish]` — build this machine's health
/// beacon from the registry's declarations and the init system's answers.
///
/// Without `--publish` the document is printed and nothing is sent, so the
/// collection can be read on a host that holds no beacon grant. Either way
/// the role strays it finds are named under `roles.unended_strays` and left
/// running: only the host process's own health pass ends them.
pub async fn collect_beacon(publish: bool) -> Result<(), CmdError> {
    let document = collect_document(Strays::Report).await?;
    super::publish_document(document, !publish).await
}

/// The `--health-interval-seconds` role of the host process: build this
/// machine's health beacon every `period` and deliver it to the fleet store,
/// promising the next one within the period plus what the last collection
/// took. The collection runs before the store is opened: it is also the
/// pass that recovers a service that stands, here or on another host, and
/// a store that stands behind that very service (an object API that
/// authorizes through the vault) kept the pass from ever running. A store
/// that cannot be opened, or a collection that fails, is said on stderr and
/// tried again on the next tick; the role never ends.
pub async fn collect_beacons_every(period: std::time::Duration) -> Result<(), CmdError> {
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut store: Option<crate::queue::JobStorage> = None;
    loop {
        schedule.tick().await;
        // Every collection runs to completion. A delayed pass skips obsolete
        // scheduled ticks instead of cancelling work or bursting repeated probes.
        let started = std::time::Instant::now();
        let collected = collect_document(Strays::End).await;
        // How long this collection took: the part of the next beacon's
        // promise the period alone does not cover, since the next one is
        // published only once its own collection finishes.
        let last_collection = started.elapsed();
        let mut document = match collected {
            Ok(document) => document,
            Err(error) => {
                eprintln!("[stado serve host-health] collect failed: {error}");
                continue;
            }
        };
        if store.is_none() {
            match crate::queue::JobStorage::new().await {
                Ok(opened) => store = Some(opened),
                Err(error) => {
                    eprintln!(
                        "[stado serve host-health] the fleet store could not be opened, so this \
                         beacon is not published; the next tick opens it again: {error}"
                    );
                    continue;
                }
            }
        }
        let Some(store) = store.as_ref() else {
            continue;
        };
        if let Err(error) = super::deliver_document(
            &mut document,
            super::Destination::Store(store),
            Some(period + last_collection),
        )
        .await
        {
            eprintln!("[stado serve host-health] publish failed: {error}");
        }
    }
}

async fn collect_document(strays: Strays) -> Result<Value, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let target = crate::providers::local::agent::lookup_self_auto(&hostname)
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is not a registry target on this fleet, so there is nothing declared \
                 here to collect a beacon about"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let runner = crate::deploy::production_runner();

    let mut units = Map::new();
    let mut verdicts = std::collections::BTreeMap::new();
    let services = crate::deploy::service::declared_services(&target);
    let mut questions = std::collections::HashMap::new();
    for service in &services {
        let unit = service.unit_id();
        if unit.is_empty() || unit.contains(char::is_whitespace) || unit.contains(',') {
            continue;
        }
        // The carrying read: launchd's event log, the costly half, is read
        // only when the unit is not as it was at the previous pass and the
        // host is under its scan load ceiling.
        let mut entry = match inspect_label_carrying(
            &target,
            unit,
            crate::deploy::service::BootoutScope::Any,
            &runner,
        )
        .await
        {
            Ok(state) => {
                verdicts.insert(unit.to_string(), (service.name.clone(), state.failing()));
                let mut entry = unit_entry(&state);
                if let Value::Object(fields) = &mut entry {
                    super::runner_listener::apply(
                        fields,
                        state.program.as_deref(),
                        state.pid.as_deref(),
                    );
                    // How the unit's launchd events were obtained this pass:
                    // `ok` for a read, `carried: …` for the previous read's.
                    if let Some(status) = &state.event_read_status {
                        fields.insert("events".to_string(), Value::String(status.clone()));
                    }
                }
                entry
            }
            // The read itself could not be run. That is this host's own
            // failure to describe, never another word for `inactive`.
            Err(error) => json!({"state": STATE_UNREADABLE, "detail": error.to_string()}),
        };
        // The question the service is for, asked at its own listen port
        // and judged against the resolver's declared refresh interval; a
        // service that stands is recovered here, on the host that runs it.
        if let Some(answering) = answering_entry(&hostname, &target, service, unit).await {
            questions.insert(answering.name, answering.asked);
            if let Value::Object(fields) = &mut entry {
                fields.insert("answering".to_string(), answering.document);
            }
        }
        units.insert(unit.to_string(), entry);
    }
    crate::deploy::service_answering::retain_declared(&questions);
    // A unit read failing is a crash loop Oko is told of once (`runtime::
    // stops::units`). What Oko answered joins the unit's `detail`, which
    // `stado service list` and Stado Desktop's Managed services show, and a
    // loop register that cannot be read or kept is said in the beacon.
    let declared = units.keys().cloned().collect();
    let unrecorded =
        match crate::cli::integrations::runtime::stops::units::observe(&declared, &verdicts) {
            Ok(reports) => {
                for (label, report) in reports {
                    if let Some(Value::Object(fields)) = units.get_mut(&label) {
                        super::runner_listener::add_detail(fields, format!("Oko defect: {report}"));
                    }
                }
                Value::Null
            }
            Err(error) => Value::String(error),
        };

    let mut document = json!({
        "host": super::beacon_slug(&hostname),
        "reported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "units": Value::Object(units),
        "crash_loops_unrecorded": unrecorded,
        // The Stado releases this host recorded as failed: a delivery to any
        // host refuses them (`release_cmd::local::failed`).
        "failed_releases": match crate::cli::release_cmd::failed_here() {
            Ok(failed) => failed,
            Err(error) => json!({ "unreadable": error.to_string() }),
        },
    });

    // The same volume the janitor measures and the same call it measures it
    // with, so a beacon and a reclaim pass can never disagree about free
    // space. `stado host health` prints these two numbers. A volume whose
    // free space cannot be read publishes no number, and its `disk` line
    // says why instead of leaving the field out unexplained.
    let home = crate::config_file::expand_tilde("~");
    if let Some(object) = document.as_object_mut() {
        match crate::providers::local::space_reclaim::free_bytes(&home) {
            Ok(free) => {
                let free_gb = free / crate::providers::local::space_reclaim::GIB;
                object.insert("disk_avail_gb".to_string(), json!(free_gb));
                object.insert(
                    "disk".to_string(),
                    Value::String(format!("{free_gb} GiB available on {}", home.display())),
                );
            }
            Err(error) => {
                object.insert(
                    "disk".to_string(),
                    Value::String(format!(
                        "free space on {} cannot be read: {error}",
                        home.display()
                    )),
                );
            }
        }
    }
    // The memory line is the host's own kernel reading, the same fields its
    // capacity publication carries.
    if let Some(object) = document.as_object_mut() {
        object.insert(
            "memory".to_string(),
            Value::Object(crate::providers::local::host_memory::publication_fields(
                &crate::providers::local::host_memory::read_host_memory(),
            )),
        );
        // What macOS lets this process read of the folders declared for it,
        // measured by this process; an undeclared folder is never opened.
        object.insert(
            "privacy".to_string(),
            super::privacy::measure(&home, &target),
        );
        // What each process spent since the reading before, so a saturated
        // host's beacon names its causes; a failed read is published as the
        // cause instead of an empty list.
        let processes = match crate::providers::local::host_processes::read_and_compare() {
            Ok(report) => serde_json::to_value(report)?,
            Err(cause) => json!({ "unread": cause }),
        };
        object.insert("processes".to_string(), processes);
        // One process per role: the roles held on this host now, and the
        // Stado role processes nothing accounts for — a second copy of a
        // role, a build run from a checkout, a program since removed, a
        // retired command. The health pass ends them and publishes them as
        // ended, so a stray never outlives one beacon; a collection run by
        // hand names them and ends none.
        object.insert(
            "roles".to_string(),
            roles_and_strays(&home, &target, strays)?,
        );
    }
    // The services of other hosts that hold this host's requests past the
    // bound, recovered from here when their own host did not act.
    let peers = crate::deploy::service_answering::peer::recoveries(&home, &target.name).await;
    if let Some(object) = document.as_object_mut() {
        object.insert("peer_recoveries".to_string(), peers);
    }
    Ok(document)
}
