//! What a beacon collection may act on beside reading: the Stado role
//! processes nothing accounts for, and the services that stand.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// Whether a collection ends the role strays it finds. Only the host
/// process's own health pass does: it runs under the host's HOME, whose role
/// locks say which processes are accounted for. A collection run by hand,
/// perhaps under another HOME (a test, a second account), finds no lock
/// there for this machine's resident and would take it for a stray.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Strays {
    /// Find them and name them in the beacon; end none.
    Report,
    /// End them, and name in the beacon the ones ended.
    End,
}

/// The `roles` block: the roles held on this host now, the Stado role
/// processes nothing accounts for — reported, or ended and reported, as
/// `strays` says — and the roles the registry declares for this process's
/// own serve unit that it does not run ([`missing_roles`]).
pub(super) fn roles_and_strays(
    home: &std::path::Path,
    target: &crate::targets::ComputeTarget,
    strays: Strays,
) -> Result<Value, CmdError> {
    let held: Vec<Value> = crate::deploy::role_lock::holders(home)?
        .into_iter()
        .map(|holder| {
            json!({
                "role": holder.role,
                "pid": holder.pid,
                "program": holder.program,
                "since": holder.since,
            })
        })
        .collect();
    let found = crate::deploy::role_lock::strays(home)?;
    if strays == Strays::Report {
        return Ok(json!({
            "held": held,
            "unended_strays": found,
            "declared_not_running": null,
        }));
    }
    let ended = crate::deploy::role_lock::end_strays(&found)?;
    for stray in &ended {
        eprintln!(
            "[stado serve host-health] ended stray pid {} ({}), running {}: {}",
            stray.pid, stray.command, stray.running_for, stray.detail
        );
    }
    let mut declared = missing_roles(target);
    let missing: Vec<String> = declared["roles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if !missing.is_empty() {
        declared["converge"] = render_again(&target.name, &missing);
    }
    Ok(json!({
        "held": held,
        "ended_strays": ended,
        "declared_not_running": declared,
    }))
}

/// The resident unit rendered again because the process running this pass,
/// which is that unit, was started without `missing`, roles its declared
/// serve unit names: the same detached `service ensure` the executable
/// starts when its unit's argument vector is refused, so one mechanism
/// renders the unit whichever way it drifted. Answers what happened, for the
/// beacon: `started` or `running` with the converge's pid and log, `none`
/// when this process is no resident unit (started by hand or under a home of
/// its own) or a converge already ran for this very line and the unit still
/// starts it, or `refused` with the cause.
fn render_again(host: &str, missing: &[String]) -> Value {
    use crate::cli::entry::dispatch::resident;
    let line = resident::own_line();
    match (resident::resident_unit_of(&line), resident::ran_for(&line)) {
        (Err(cause), _) | (_, Err(cause)) => return json!({ "refused": cause }),
        (Ok(None), _) => return json!({ "none": "this process is no resident unit of this host" }),
        (Ok(Some(_)), Ok(true)) => {
            return json!({
                "none": "a converge already ran for this process's line and the unit still starts it"
            })
        }
        (Ok(Some(_)), Ok(false)) => {}
    }
    let reason = format!(
        "the registry declares serve roles the resident unit does not run: {}",
        missing.join(" ")
    );
    match resident::start(host, &reason, &line) {
        Ok(started) => match started.state {
            resident::Converging::Running(pid) => json!({ "running": pid, "log": started.log }),
            resident::Converging::Started(pid) => json!({ "started": pid, "log": started.log }),
        },
        Err(cause) => json!({ "refused": cause }),
    }
}

/// The `stado serve` roles the registry declares for this process's own
/// serve unit that it does not run, read from both command lines by serve's
/// own definition. A unit rewritten without a declared role keeps answering
/// while that role stands silent: a unit without `--resolver` serves its API
/// while every reader of the registry it would refresh goes stale, and
/// nothing else says why. The units compared are the
/// declared serve lines that take this process's role lock (`serve`, or
/// `serve--<target>`), since only one process holds a lock: a serve unit for
/// another worker target, or a unit running another Stado command, is
/// another process and is not compared. When no declared line takes the
/// lock, nothing was compared and the answer is `no_declared_unit` with the
/// lock. Measured only in the host process's own health pass, whose
/// arguments are the unit's; a collection run by hand reports `null` above.
fn missing_roles(target: &crate::targets::ComputeTarget) -> Value {
    use crate::cli::integrations::runtime::roles::{command_roles, serve_lock};
    // The line as the converge reads it, program first; the roles are read
    // from the words after the program.
    let line = crate::cli::entry::dispatch::resident::own_line();
    let own: Vec<&str> = line.iter().skip(1).map(String::as_str).collect();
    let (running, lock) = match (command_roles(&own), serve_lock(&own)) {
        (Ok(roles), Ok(Some(lock))) => (roles, lock),
        (Ok(_), Ok(None)) => {
            let cause = format!("this process does not run `stado serve`: {}", own.join(" "));
            return json!({ "unread": cause });
        }
        (Err(cause), _) | (_, Err(cause)) => {
            return json!({ "unread": format!("this process's own arguments: {cause}") })
        }
    };
    let mut missing: Vec<&'static str> = Vec::new();
    let mut units = Vec::new();
    let mut unreadable = Vec::new();
    for service in crate::deploy::service::declared_services(target) {
        let words: Vec<&str> = service.args.iter().map(String::as_str).collect();
        let unit = service.unit_id().to_string();
        let declared = match serve_lock(&words) {
            Ok(Some(declared)) if declared == lock => command_roles(&words),
            Ok(_) => continue,
            Err(cause) => Err(cause),
        };
        match declared {
            Ok(declared) => {
                for role in declared {
                    if !running.contains(&role) && !missing.contains(&role) {
                        missing.push(role);
                    }
                }
                units.push(unit);
            }
            Err(cause) => unreadable.push(json!({ "unit": unit, "cause": cause })),
        }
    }
    // Nothing compared is said as such, never as every role running.
    if units.is_empty() && unreadable.is_empty() {
        return json!({ "no_declared_unit": lock });
    }
    json!({ "roles": missing, "declared_by": units, "unreadable": unreadable })
}

pub(super) struct Answering {
    pub name: String,
    pub document: Value,
    /// The service had a usable declaration, port and standing bound.
    pub asked: bool,
}

fn unreadable(name: String, cause: impl Into<String>) -> Answering {
    Answering {
        name,
        document: crate::deploy::service_answering::Reading::Unreadable {
            cause: cause.into(),
        }
        .document(),
        asked: false,
    }
}

/// The `answering` block of one unit: the declared question asked at the
/// service's recorded listen port, the reading of it against the
/// resolver's declared refresh interval, and the recovery attempted for
/// this failed-answering episode. An already recovered episode reports its
/// last attempt instead of restarting again. `None` for a service whose
/// catalog entry declares no question. A missing directory port or resolver
/// interval is named rather than guessed.
pub(super) async fn answering_entry(
    hostname: &str,
    target: &crate::targets::ComputeTarget,
    service: &crate::deploy::service::ManagedService,
    unit: &str,
) -> Option<Answering> {
    let entry = match crate::deploy::service_catalog::lookup(&service.name) {
        Ok(Some(entry)) => entry,
        Ok(None) => return None,
        Err(error) => return Some(unreadable(service.name.clone(), error.to_string())),
    };
    let probe = entry.answering.as_ref()?;
    let port = match crate::cli::directory::recorded_listen_port(&entry, &target.name).await {
        Ok(Some(port)) => port,
        Ok(None) => {
            let cause = format!(
                "the service directory records no listen port for {} on {hostname}",
                entry.name
            );
            return Some(unreadable(entry.name, cause));
        }
        Err(error) => return Some(unreadable(entry.name, error.to_string())),
    };
    let bound_seconds = match crate::cli::resolver::declared_bound_seconds() {
        Ok(Some(seconds)) => seconds,
        Ok(None) => {
            return Some(unreadable(
                entry.name,
                "this host's resolver declares no directory refresh interval, the bound a standing question is judged by"
            ))
        }
        Err(cause) => return Some(unreadable(entry.name, cause)),
    };
    let reading =
        crate::deploy::service_answering::ask_and_read(&entry, unit, port, bound_seconds)?;
    crate::deploy::service_answering::episode::observe(&entry.name, &reading);
    let mut block = reading.document();
    if reading.stands() {
        if let Some(previous) =
            crate::deploy::service_answering::episode::begin(&entry.name, unit, port, probe)
        {
            crate::deploy::service_answering::forget(&entry.name);
            block["recovery"] = previous;
            return Some(Answering {
                name: entry.name,
                document: block,
                asked: true,
            });
        }
        let home = crate::config_file::expand_tilde("~");
        let recovered = crate::deploy::service_answering::recover::recover(
            &home, hostname, &entry, unit, &reading, None,
        )
        .await;
        crate::deploy::service_answering::forget(&entry.name);
        let record = match recovered {
            Ok(record) => record,
            Err(error) => json!({ "refusal": error.to_string() }),
        };
        crate::deploy::service_answering::episode::finish(
            &entry.name,
            unit,
            port,
            probe,
            record.clone(),
        );
        block["recovery"] = record;
    }
    Some(Answering {
        name: entry.name,
        document: block,
        asked: true,
    })
}
