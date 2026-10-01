//! `billing watch` — one watchdog pass.
//!
//! The pass body is deliberately small and the pieces around it are the
//! components: `mail` runs the fault-isolated sweep of provider notices
//! Skrzynka received, `constants` says which mail counts as one, `report`
//! emits the pass (JSON document or human tables), and `render` holds the
//! tables themselves. The pass owns only the order the three storage
//! operations happen in; the schedule that runs it is its cadence.

mod constants;
mod mail;
mod render;
mod report;

use chrono::Utc;

use crate::cli::CmdError;
use crate::monitor::billing;
use crate::queue::JobStorage;

use mail::mail_probe;
use report::report;

/// One pass: refresh the snapshot, evaluate BOTH the balance thresholds and
/// account health, dispatch only the conditions that just became true, and
/// print the report. The de-duplication state is the previous snapshot in
/// the blob, shared with a coordinator tick running in parallel; a pass that
/// cannot read it says so and fails, because evaluating against nothing
/// would re-fire every standing condition.
pub(super) async fn watch(store: &JobStorage, as_json: bool) -> Result<(), CmdError> {
    let previous = billing::load_snapshot(store).await.map_err(|err| {
        CmdError::click(format!(
            "billing history unreadable, so no transition can be told from a standing condition: {err}"
        ))
    })?;
    let mut document = billing::live_snapshot(store).await;
    let evaluation = billing::apply_health(previous.as_ref(), &mut document, Utc::now());
    billing::commit_firing(&mut document, &evaluation);
    billing::persist_snapshot(store, &document).await.map_err(|err| {
        CmdError::click(format!(
            "billing snapshot could not be stored, so its alerts were not sent: {err}"
        ))
    })?;
    billing::dispatch_signals(&evaluation).await;
    let mail = mail_probe().await;
    report(&document, &evaluation, &mail, as_json)
}
