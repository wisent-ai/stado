//! Billing-credits collector.
//!
//! Port of `stado/monitor/billing.py`. Each Cloud Function tick this writes
//! gs://<BUCKET>/billing_health/credits.json following the exact convention
//! of host_health/<host>.json: a single JSON blob, an ISO-8601 reported_at,
//! and per-source sections that either carry data or the EXACT upstream
//! error (never a mock, never a silent skip — a failed source records its
//! real status/detail so the cause is visible without log spelunking).
//!
//! GCP section: derived entirely from the BigQuery billing export. Gross
//! cost, credits applied (negative), net cost, per-credit cumulative
//! consumption, and a 7-day credit burn rate. The depletion signal is the
//! latest-month net_cost crossing BILLING_NET_ALERT_USD — this needs no
//! knowledge of the original grant ceiling, which no GCP API exposes, so the
//! tracker stays fully automated.
//!
//! Azure section: available credit balance via the ARM REST API,
//! authenticated with a service principal whose JSON value is read only from
//! the separate Skarbiec service. Local files, process-environment secrets,
//! queue blobs, Azure Key Vault, and GCP Secret Manager are not credential
//! sources. A missing service, grant, or item is explicit in the provider
//! section; there is no substitute path that can silently re-couple clouds.
//!
//! Transport deviations from Python (same on-the-wire data):
//! - Python uses the google-cloud-bigquery library; here the queries run
//!   over the BigQuery REST `queries` endpoint, so rows are parsed from the
//!   REST `rows[].f[].v` shape (all values string-typed) instead of the
//!   library's typed Row objects. The three SQL strings are byte-identical.
//! - The Python implementation's GCP Secret Manager credential lookup is
//!   replaced by an action-scoped request to the separate Skarbiec service.
//!   This removes the cross-cloud credential dependency.
//! - Python's `ClientSecretCredential` is the literal OAuth2 client-
//!   credentials POST to login.microsoftonline.com.
//! - Python records exception detail as `{type(e).__name__}: {e}`; Rust has
//!   no exception class names, so the detail is the error's Display text
//!   (the exact upstream error is preserved either way).
//!
//! Account health (NO Python original — this is new in the Rust runtime).
//! The credit signals above are computed INSIDE the `ok` branch of a
//! provider section: `credit_depleted` and `available_balance` only exist
//! when the query actually succeeded. So the moment an account is closed,
//! its billing export is revoked, or its service principal is disabled, the
//! section flips to `no_credentials`/`error`, every balance field vanishes,
//! and the balance alerts go quiet — the monitoring falls silent precisely
//! when it matters. That is how the GCP billing outage arrived with zero
//! warning.
//!
//! [`apply_health`] therefore folds a per-provider health record forward
//! across ticks inside the same blob ([`HEALTH_KEY`]): the last `ok`
//! timestamp, the start of the current failing run, and its length, so
//! "how long has this been broken" is answerable from the snapshot alone.
//! A section non-`ok` for longer than [`HEALTH_GRACE_SECONDS`] raises its
//! own alert naming the provider and the exact upstream cause, entirely
//! independent of any balance threshold. Every condition is keyed
//! ([`Signal::key`]) and the firing set is persisted, so a failure that
//! stays broken alerts on the transition rather than once per poll.
//!
//! The components are the sources this file already read from: [`gcp`] holds
//! the BigQuery billing-export collector, [`azure`] the Skarbiec
//! service-principal read plus the ARM credit, grant and billing-property
//! reads, [`health`] the account-health fold and the alerting it drives, and
//! [`format`] the renderers they share. Snapshot assembly and publication
//! stay here. Every name a caller outside this module uses is re-exported
//! here, so `crate::monitor::billing::<item>` resolves exactly as before.

mod azure;
mod format;
mod gcp;
mod health;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use crate::config;
use crate::queue::{JobStorage, StorageError};

use azure::azure_section;
use format::{error_section, log};
use gcp::gcp_section;
use health::emit_alerts;

pub use health::{
    apply_health, commit_firing, dispatch_signals, humanize, providers, HealthEvaluation,
    ProviderHealth, Signal, HEALTH_GRACE_SECONDS, HEALTH_KEY, SECONDS_PER_DAY, SECONDS_PER_HOUR,
    SECONDS_PER_MINUTE, SECONDS_PER_SECOND,
};

/// Blob written every tick (Python `_BLOB`).
pub const BLOB: &str = "billing_health/credits.json";

// ---------------------------------------------------------------------------
// collect_billing
// ---------------------------------------------------------------------------

/// Query every billing source and return the canonical snapshot without
/// persisting it. Used by both the coordinator collector and `billing
/// refresh`. `store` remains the eventual snapshot destination; the Azure
/// service-principal credential is resolved independently from Azure Key
/// Vault.
pub async fn live_snapshot(store: &JobStorage) -> Value {
    let variants = crate::capabilities::get(crate::capabilities::RuntimeFacet::Billing.as_str())
        .map(|capability| capability.variants)
        .unwrap_or_default();
    let enabled = config::billing_providers();
    let sections = futures::future::join_all(
        variants
            .iter()
            .filter(|variant| enabled.iter().any(|provider| provider == variant.id))
            .map(|variant| async move {
                let value = match variant.adapter {
                    crate::capabilities::RuntimeAdapter::Billing(
                        crate::capabilities::BillingAdapter::Gcp,
                    ) => gcp_section().await,
                    crate::capabilities::RuntimeAdapter::Billing(
                        crate::capabilities::BillingAdapter::Azure,
                    ) => azure_section(store).await,
                    _ => error_section(format!(
                        "billing catalog variant {:?} has no billing adapter",
                        variant.id
                    )),
                };
                (variant.id, value)
            }),
    )
    .await;
    billing_document_from_sections(sections)
}

fn billing_document_from_sections(
    sections: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut document = serde_json::Map::new();
    document.insert(
        "reported_at".to_string(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)),
    );
    document.insert(
        "project".to_string(),
        Value::String(config::project().to_string()),
    );
    for (provider, section) in sections {
        document.insert(provider.to_string(), section);
    }
    Value::Object(document)
}

fn billing_document(gcp: Value, azure: Value) -> Value {
    billing_document_from_sections([
        (crate::capabilities::ProviderId::Gcp.as_str(), gcp),
        (crate::capabilities::ProviderId::Azure.as_str(), azure),
    ])
}

/// Persist one already-built billing document.
pub async fn persist_snapshot(store: &JobStorage, document: &Value) -> Result<(), StorageError> {
    let pretty = serde_json::to_string_pretty(document).expect("json value serializes");
    store.upload_text(BLOB, &pretty).await
}

/// The last published snapshot, or `None` when the blob has never been
/// written. Unparseable JSON also reads as `None`: a corrupt record must
/// never stop the next tick from publishing a good one.
pub async fn load_snapshot(store: &JobStorage) -> Result<Option<Value>, StorageError> {
    Ok(store
        .download_text(BLOB)
        .await?
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Assemble and upload billing_health/credits.json. Each source is isolated:
/// a failure from one is captured into its section as the exact error string.
pub async fn collect_billing(store: &JobStorage) {
    publish(store, live_snapshot(store).await).await;
}

/// Compatibility helper for callers that already hold provider sections.
pub async fn write_billing_blob(store: &JobStorage, gcp: Value, azure: Value) {
    publish(store, billing_document(gcp, azure)).await;
}

/// Fold health forward, commit the firing set, persist, log, and dispatch
/// the transitions. Every alerting caller (the coordinator collector and
/// `stado billing watch`) goes through here, so the de-duplication state in
/// the blob has exactly one writer discipline.
async fn publish(store: &JobStorage, mut document: Value) {
    let previous = match load_snapshot(store).await {
        Ok(previous) => previous,
        Err(err) => {
            // A history read failure must not suppress this tick; it only
            // costs the elapsed-time context on any alert it raises.
            log(&format!("billing history unreadable: {err}"));
            None
        }
    };
    let evaluation = apply_health(previous.as_ref(), &mut document, Utc::now());
    commit_firing(&mut document, &evaluation);
    if let Err(err) = persist_snapshot(store, &document).await {
        log(&format!("billing upload failed: {err}"));
        return;
    }
    emit_alerts(&document);
    dispatch_signals(&evaluation).await;
}
