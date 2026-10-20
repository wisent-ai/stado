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

/// The `roles` block: the roles held on this host now, and the Stado role
/// processes nothing accounts for — reported, or ended and reported, as
/// `strays` says.
pub(super) fn roles_and_strays(home: &std::path::Path, strays: Strays) -> Result<Value, CmdError> {
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
        }));
    }
    let ended = crate::deploy::role_lock::end_strays(&found)?;
    for stray in &ended {
        eprintln!(
            "[stado serve host-health] ended stray pid {} ({}), running {}: {}",
            stray.pid, stray.command, stray.running_for, stray.detail
        );
    }
    Ok(json!({
        "held": held,
        "ended_strays": ended,
    }))
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
