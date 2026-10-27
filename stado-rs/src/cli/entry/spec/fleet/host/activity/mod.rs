//! `stado host activity TARGET`: what a host did, from the queue's records.

use clap::Subcommand;
use serde_json::Value;

use crate::cli::CmdError;

#[derive(Subcommand)]
pub(crate) enum HostActivityCommands {
    /// Every job TARGET's worker ran, oldest first, from the queue's own
    /// records: when it was created, started and ended, its state, command,
    /// worker kind and seconds. `stado host health` and `stado host link`
    /// say what the host is now; this says what it did.
    Activity {
        /// Registry target name.
        target: String,
        /// Only jobs whose end, start or creation is at or after this RFC
        /// 3339 time.
        #[arg(long)]
        since: Option<String>,
        /// Print the timeline as JSON instead of one line per job.
        #[arg(long)]
        json: bool,
    },
}

/// One field of a timeline entry as a person reads it; a field the record
/// does not state is said as such.
fn said(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "(not recorded)".to_string(),
        Some(other) => other.to_string(),
    }
}

/// Answer `stado host activity`.
pub(crate) async fn dispatch(command: HostActivityCommands) -> Result<(), CmdError> {
    let HostActivityCommands::Activity {
        target,
        since,
        json,
    } = command;
    let since = match since.as_deref() {
        None => None,
        Some(text) => Some(chrono::DateTime::parse_from_rfc3339(text).map_err(|error| {
            CmdError::usage(format!(
                "--since {text:?} is not an RFC 3339 time ({error}); write it as \
                 2026-01-31T00:00:00Z"
            ))
        })?),
    };
    let host = crate::cli::canonical_host(&target).await?;
    let store = crate::queue::JobStorage::new()
        .await
        .map_err(CmdError::from)?;
    let timeline = crate::monitor::host_activity::read(&store, &host, since)
        .await
        .map_err(CmdError::from)?;
    if json {
        return crate::cli::print_answer(&timeline, true);
    }
    let Some(jobs) = timeline.get("jobs").and_then(Value::as_array) else {
        return Err(CmdError::unreachable(
            "the activity timeline carries no job list",
        ));
    };
    if jobs.is_empty() {
        println!("{target}: no job of the queue names this host as its worker");
    }
    for job in jobs {
        println!(
            "{}  {}  {}  {}s  {}",
            said(job.get("at")),
            said(job.get("lifecycle")),
            said(job.get("job")),
            said(job.get("seconds")),
            said(job.get("command"))
        );
    }
    println!(
        "{} job(s), {} measured second(s); records naming no worker: {}",
        jobs.len(),
        said(timeline.get("seconds")),
        said(timeline.get("records_without_allocation"))
    );
    Ok(())
}
