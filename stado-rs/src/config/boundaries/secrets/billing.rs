//! Billing sources, export coordinates and provider credential items.

use std::sync::LazyLock;

use crate::config::canonicalize_capability_names;
use crate::config_file::{resolve as cfg_value, resolve_list as cfg_list};

static BILLING_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Billing,
        cfg_list(
            "WC_BILLING_PROVIDERS",
            "billing.providers",
            &["gcp", "azure"],
        ),
    )
});
// No export coordinate or threshold is built in: the export table is named
// after one billing account, and the threshold is a sum of money. Both used
// to be, so an undeclared deployment queried another account's export and
// judged its credit against an amount nobody chose.
static BILLING_DATASET: LazyLock<String> =
    LazyLock::new(|| cfg_value("WC_BILLING_DATASET", "billing.dataset", ""));
static BILLING_TABLE: LazyLock<String> =
    LazyLock::new(|| cfg_value("WC_BILLING_TABLE", "billing.table", ""));
static BILLING_NET_ALERT_USD: LazyLock<Result<f64, String>> = LazyLock::new(|| {
    let raw = cfg_value("WC_BILLING_NET_ALERT_USD", "billing.net_alert_usd", "");
    if raw.trim().is_empty() {
        return Err(
            "WC_BILLING_NET_ALERT_USD is not declared: the net spend in USD at which \
                    credit counts as exhausted is the deployment's to say"
                .to_string(),
        );
    }
    raw.trim()
        .parse::<f64>()
        .map_err(|error| format!("WC_BILLING_NET_ALERT_USD must be a number, not {raw}: {error}"))
});

/// Billing sources queried by the collector. This is independent from compute
/// provider enablement: an account may stay fenced for provisioning while its
/// spend and grant state remain monitored.
pub fn billing_providers() -> &'static [String] {
    &BILLING_PROVIDERS
}

/// BigQuery billing export dataset (env `WC_BILLING_DATASET`, config key
/// `billing.dataset`); empty when undeclared.
///
/// The collector reads gross spend, applied credits, net spend and burn from
/// the configured billing export, alongside provider credit balances. The
/// export table is account-specific, so its dataset and table are deployment
/// inputs rather than values callers should reproduce in code.
pub fn billing_dataset() -> &'static str {
    BILLING_DATASET.as_str()
}

/// BigQuery billing export table (env `WC_BILLING_TABLE`, config key
/// `billing.table`); empty when undeclared.
pub fn billing_table() -> &'static str {
    BILLING_TABLE.as_str()
}

/// Net-spend alert threshold in USD (env `WC_BILLING_NET_ALERT_USD`, config
/// key `billing.net_alert_usd`), or why there is none. A day whose net_cost
/// (gross + credits, credits are negative) exceeds this means the promotion
/// credit no longer fully covers spend — i.e. it is exhausted or rate-capped.
/// This is the depletion signal; it needs no knowledge of the original grant
/// ceiling (which no GCP API exposes).
pub fn billing_net_alert_usd() -> Result<f64, &'static str> {
    BILLING_NET_ALERT_USD
        .as_ref()
        .copied()
        .map_err(String::as_str)
}

/// The Skarbiec role whose item holds the Azure billing service principal as
/// `{"tenant_id","client_id","client_secret", ...}`. Its value has no
/// alternative source.
pub const AZURE_BILLING_ROLE: &str = "azure-billing";

/// The Skarbiec role whose item holds the Azure provider service principal as
/// `{"tenant_id","client_id","client_secret"}`, used by hosts that have no
/// Azure managed identity: the role every cloud provider credential plays,
/// [`crate::capabilities::cloud_credential_role`].
///
/// A managed identity is still preferred and tried first; this exists because
/// the control plane runs on hardware outside Azure, where IMDS answers
/// nothing and Azure Blob would otherwise be unreachable.
pub fn azure_provider_role() -> String {
    crate::capabilities::cloud_credential_role(crate::capabilities::ProviderId::Azure)
}
