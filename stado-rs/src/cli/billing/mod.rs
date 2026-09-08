//! `stado billing show | refresh | watch` — the operator surface over
//! `billing_health/credits.json`.
//!
//! NO Python original for `watch`: the Python CLI has `show` and `refresh`
//! only, and the collector runs exclusively as a Cloud Function tick inside
//! the very GCP project it is measuring.
//!
//! That co-location is the defect this command exists to fix, and it is
//! deliberate that `billing watch` is a FOREGROUND process runnable from
//! anywhere — a laptop, a host in `registry.json`, another cloud. A
//! collector that dies with its provider cannot warn you about that
//! provider. When the GCP billing account was shut off, the Cloud Function
//! publishing `billing_health/credits.json` was shut off with it, so the
//! blob simply stopped changing and nothing anywhere raised a sound. Run
//! this OUTSIDE the cloud it monitors and the watchdog survives the outage
//! it is watching for.
//!
//! Two independent conditions are evaluated every poll (see
//! `monitor/billing.rs::signals`): the credit/balance thresholds, which
//! only exist while a provider section is `ok`, and account health, which
//! is what speaks when a section is `no_credentials` or `error` and no
//! balance figure exists at all. Alerts fire on the TRANSITION into a
//! condition — the firing set lives in the blob, so a failure that stays
//! broken does not re-alert every poll, and the de-duplication survives
//! both a restart of this process and a concurrent coordinator tick.
//!
//! Mail is wired in as advisory evidence: providers announce closure,
//! failed payment and credit expiry by email days before the API starts
//! refusing calls. The sweep reuses `cli/mail.rs`'s read-only Gmail client
//! and is fault-isolated — no Gmail token, no scope, or a dead Gmail never
//! fails the watch, it only prints why the evidence is missing.
//!
//! The components follow the verbs and the reads: `show` renders a
//! published snapshot for a human, `watch` holds the foreground watchdog
//! with its mail sweep and per-poll report, `interval` is the clap parser
//! for `--interval`, and `format` is the scalar renderer the two output
//! paths share. Command dispatch, the `--json` emitter and `refresh` stay
//! here, and `parse_interval` is re-exported so the clap spec resolves
//! `billing::parse_interval` exactly as before.

mod format;
mod interval;
mod show;
mod watch;

use chrono::Utc;
use serde_json::{json, Value};

use super::{BillingCommands, CmdError};
use crate::monitor::billing;
use crate::queue::JobStorage;

use show::print_human;
use watch::watch;

pub use interval::parse_interval;

pub(crate) async fn dispatch(command: &BillingCommands) -> Result<(), CmdError> {
    let store = JobStorage::with_bucket(crate::config::bucket()).await?;
    match command {
        BillingCommands::Show { json } => {
            let document = match store.download_text(billing::BLOB).await? {
                Some(text) => serde_json::from_str(&text)?,
                None => json!({
                    "status": "unavailable",
                    "detail": format!("{} has not been published yet; run stado billing refresh", billing::BLOB),
                }),
            };
            emit(&document, *json)
        }
        BillingCommands::Refresh { json } => {
            let document = refresh(&store).await;
            emit(&document, *json)
        }
        BillingCommands::Watch {
            interval,
            once,
            json,
        } => watch(&store, *interval, *once, *json).await,
    }
}

fn emit(document: &Value, as_json: bool) -> Result<(), CmdError> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(document)?);
    } else {
        print_human(document);
    }
    Ok(())
}

/// Query the providers now and republish the snapshot.
///
/// The health record is folded forward but the firing set is NOT committed
/// and nothing is dispatched: a hand-run refresh must not consume the alert
/// transition the collector or `billing watch` still owes (see
/// `monitor/billing.rs::apply_health`). Skipping the fold entirely would be
/// worse than either — the republished document would carry no health
/// record, erasing every provider's last-good timestamp.
async fn refresh(store: &JobStorage) -> Value {
    let previous = match billing::load_snapshot(store).await {
        Ok(previous) => previous,
        Err(err) => {
            eprintln!("Warning: billing history unreadable: {err}");
            None
        }
    };
    let mut document = billing::live_snapshot(store).await;
    billing::apply_health(previous.as_ref(), &mut document, Utc::now());
    if let Err(err) = billing::persist_snapshot(store, &document).await {
        eprintln!("Warning: live billing data could not be cached: {err}");
    }
    document
}
