//! `billing watch` — one watchdog pass.
//!
//! The pass body is deliberately small and the pieces around it are the
//! components: `mail` runs the fault-isolated sweep of provider notices
//! Skrzynka received, `constants` says which mail counts as one, `notices`
//! records each notice that asks for action as an Oko defect once, `report`
//! emits the pass (JSON document or human tables), and `render` holds the
//! tables themselves. The pass owns only the order the storage operations
//! happen in; the schedule that runs it is its cadence.

mod constants;
mod mail;
mod notices;
mod render;
mod report;

use chrono::Utc;

use crate::cli::CmdError;
use crate::monitor::billing;
use crate::queue::JobStorage;

use mail::mail_probe;
use report::report;

/// One pass: refresh the snapshot, evaluate BOTH the balance thresholds and
/// account health, record every provider notice that asks for action as a
/// defect of that dependency, dispatch only the conditions that just became
/// true, and print the report. The de-duplication state is the previous
/// snapshot in the blob, shared with a coordinator tick running in parallel;
/// a pass that cannot read it says so and fails, because evaluating against
/// nothing would re-fire every standing condition and record every notice
/// again.
pub(super) async fn watch(
    store: &JobStorage,
    mail_days: u32,
    mail_organization: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let previous = billing::load_snapshot(store).await.map_err(|err| {
        let message = format!(
            "billing history unreadable, so no transition can be told from a standing condition: {err}"
        );
        let mut failed = CmdError::from(err);
        failed.message = Some(message);
        failed
    })?;
    let mut document = billing::live_snapshot(store).await;
    let evaluation = billing::apply_health(previous.as_ref(), &mut document, Utc::now());
    billing::commit_firing(&mut document, &evaluation);
    // The notices are read before the snapshot is stored: a notice recorded
    // as a defect is written into the snapshot, so the next pass, which reads
    // the same mail again, records it no second time.
    let mail = mail_probe(mail_days, mail_organization).await;
    let recorded = notices::record(previous.as_ref(), &mut document, &mail);
    billing::persist_snapshot(store, &document)
        .await
        .map_err(|err| {
            let message =
                format!("billing snapshot could not be stored, so its alerts were not sent: {err}");
            let mut failed = CmdError::from(err);
            failed.message = Some(message);
            failed
        })?;
    billing::dispatch_signals(&evaluation).await;
    report(&document, &evaluation, &mail, &recorded, as_json)
}
