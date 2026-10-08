//! Billing sources, export coordinates and provider credential items.

use std::sync::LazyLock;

use crate::config::canonicalize_capability_names;
use crate::config_file::resolve_list as cfg_list;

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
static BILLING_DATASET: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_BILLING_DATASET").unwrap_or_else(|_| "billing_export".to_string())
});
static BILLING_TABLE: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_BILLING_TABLE")
        .unwrap_or_else(|_| "gcp_billing_export_v1_017364_D3B657_F207B5".to_string())
});
static BILLING_NET_ALERT_USD: LazyLock<f64> = LazyLock::new(|| {
    std::env::var("WC_BILLING_NET_ALERT_USD")
        .unwrap_or_else(|_| "100".to_string())
        .parse::<f64>()
        .expect("WC_BILLING_NET_ALERT_USD must be a number")
});

/// Billing sources queried by the collector. This is independent from compute
/// provider enablement: an account may stay fenced for provisioning while its
/// spend and grant state remain monitored.
pub fn billing_providers() -> &'static [String] {
    &BILLING_PROVIDERS
}

/// BigQuery billing export dataset (env `WC_BILLING_DATASET`).
///
/// The collector reads gross spend, applied credits, net spend and burn from
/// the configured billing export, alongside provider credit balances. The
/// export table is account-specific, so its dataset and table are deployment
/// inputs rather than values callers should reproduce in code.
pub fn billing_dataset() -> &'static str {
    BILLING_DATASET.as_str()
}

/// BigQuery billing export table (env `WC_BILLING_TABLE`).
pub fn billing_table() -> &'static str {
    BILLING_TABLE.as_str()
}

/// Net-spend alert threshold in USD (env `WC_BILLING_NET_ALERT_USD`). A day
/// whose net_cost (gross + credits, credits are negative) exceeds this
/// means the promotion credit no longer fully covers spend — i.e. it is
/// exhausted or rate-capped. This is the depletion signal; it needs no
/// knowledge of the original grant ceiling (which no GCP API exposes).
pub fn billing_net_alert_usd() -> f64 {
    *BILLING_NET_ALERT_USD
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
