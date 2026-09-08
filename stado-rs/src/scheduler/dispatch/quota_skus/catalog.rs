//! Provider dispatch for the read side: one provider's catalog, then the
//! map over every configured provider.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::azure::azure_catalog;
use super::client::{CatalogError, CloudQuotasClient};
use super::gcp::{gcp_catalog, gcp_project_env};

/// Return the full GPU catalog for `provider` (gcp | azure) — Python
/// `provider_catalog`. `gcp_client` is injectable for tests; `None`
/// resolves a live client for the gcp arm.
pub async fn provider_catalog(
    provider: &str,
    gcp_client: Option<&CloudQuotasClient>,
) -> Result<Vec<Value>, CatalogError> {
    let adapter = crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, provider)
        .map(|variant| variant.adapter);
    match adapter {
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Gcp,
        )) => {
            let owned;
            let client = match gcp_client {
                Some(client) => client,
                None => {
                    owned = CloudQuotasClient::new(&gcp_project_env()).await?;
                    &owned
                }
            };
            gcp_catalog(client).await
        }
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Azure,
        )) => Ok(azure_catalog().await),
        _ => Ok(vec![json!({
            "provider": provider,
            "ok": false,
            "error": "no catalog impl for this provider",
        })]),
    }
}

/// provider_name -> list of catalog rows (Python `all_catalogs`).
/// Deviation: the Rust map is BTreeMap-ordered (alphabetical) where the
/// Python dict preserves the input `providers` order; the CLI's --json
/// output sorts keys anyway.
pub async fn all_catalogs(
    providers: &[String],
    gcp_client: Option<&CloudQuotasClient>,
) -> Result<BTreeMap<String, Vec<Value>>, CatalogError> {
    let mut out = BTreeMap::new();
    for provider in providers {
        out.insert(
            provider.clone(),
            provider_catalog(provider, gcp_client).await?,
        );
    }
    Ok(out)
}
