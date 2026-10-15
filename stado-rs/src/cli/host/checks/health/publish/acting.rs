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

/// The `answering` block of one unit: the declared question asked at the
/// service's recorded listen port, the reading of it against the
/// resolver's declared refresh interval, and, for a service that stands,
/// the recovery this pass ran. `None` for a service whose catalog entry
/// declares no question. A port the directory does not record, or a
/// resolver that declares no interval, is said in the block instead of
/// being guessed.
pub(super) async fn answering_entry(
    hostname: &str,
    target: &crate::targets::ComputeTarget,
    service: &crate::deploy::service::ManagedService,
    unit: &str,
) -> Option<Value> {
    let entry = match crate::deploy::service_catalog::lookup(&service.name) {
        Ok(Some(entry)) => entry,
        Ok(None) => return None,
        Err(error) => return Some(json!({ "unreadable": error })),
    };
    entry.answering.as_ref()?;
    let port = match crate::cli::directory::recorded_listen_port(&entry, &target.name).await {
        Ok(Some(port)) => port,
        Ok(None) => {
            return Some(json!({
                "unreadable": format!("the service directory records no listen port for {} on {hostname}", entry.name)
            }))
        }
        Err(error) => return Some(json!({ "unreadable": error.to_string() })),
    };
    let bound_seconds = match crate::cli::resolver::declared_bound_seconds() {
        Ok(Some(seconds)) => seconds,
        Ok(None) => {
            return Some(json!({
                "unreadable": "this host's resolver declares no directory refresh interval, the bound a standing question is judged by"
            }))
        }
        Err(unreadable) => return Some(json!({ "unreadable": unreadable })),
    };
    let reading = crate::deploy::service_answering::ask_and_read(&entry, port, bound_seconds)?;
    let mut block = reading.document();
    if reading.stands() {
        let home = crate::config_file::expand_tilde("~");
        let recovered = crate::deploy::service_answering::recover::recover(
            &home, hostname, &entry, unit, &reading, None,
        )
        .await;
        crate::deploy::service_answering::forget(&entry.name);
        block["recovery"] = match recovered {
            Ok(record) => record,
            Err(error) => json!({ "refusal": error.to_string() }),
        };
    }
    Some(block)
}
