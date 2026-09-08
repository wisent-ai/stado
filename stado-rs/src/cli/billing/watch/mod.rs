//! `billing watch` — the foreground watchdog loop.
//!
//! The poll body is deliberately small and the pieces around it are the
//! components: `mail` runs the fault-isolated Gmail sweep, `report` emits
//! the poll (JSON document or human tables), and `render` holds the tables
//! themselves. The loop below owns only the de-duplication state and the
//! order the three storage operations happen in.

mod mail;
mod render;
mod report;

use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

use crate::cli::CmdError;
use crate::monitor::billing;
use crate::queue::JobStorage;

use mail::mail_probe;
use report::report;

/// Foreground watchdog. Each poll refreshes the snapshot, evaluates BOTH
/// the balance thresholds and account health, dispatches only the
/// conditions that just became true, and prints a status line.
pub(super) async fn watch(
    store: &JobStorage,
    interval: Duration,
    once: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // De-duplication state is read back from the blob every poll so a
    // coordinator tick running in parallel shares it. The in-memory copy is
    // only a stand-in for a storage read failure, which must not turn a
    // single persistent fault into an alert storm.
    let mut last: Option<Value> = None;
    loop {
        let previous = match billing::load_snapshot(store).await {
            Ok(Some(document)) => Some(document),
            Ok(None) => last.take(),
            Err(err) => {
                eprintln!("Warning: billing history unreadable: {err}");
                last.take()
            }
        };
        let mut document = billing::live_snapshot(store).await;
        let evaluation = billing::apply_health(previous.as_ref(), &mut document, Utc::now());
        billing::commit_firing(&mut document, &evaluation);
        if let Err(err) = billing::persist_snapshot(store, &document).await {
            // Uncommitted state re-alerts next poll rather than losing the
            // transition — the safe direction for a billing watchdog.
            eprintln!("Warning: billing snapshot could not be cached: {err}");
        }
        billing::dispatch_signals(&evaluation).await;

        let mail = mail_probe().await;
        report(&document, &evaluation, &mail, as_json)?;
        last = Some(document);

        if once {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
    }
}
