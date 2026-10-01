//! The GCP price read: the Cloud Billing Catalog, paged, one quote per SKU
//! and service region, each classified by the Pricing API's product taxonomy.

mod taxonomy;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::capabilities::ProviderId;

use super::{PriceQuote, PriceSource, PriceState};
use taxonomy::{gcp_sku_taxonomy, SkuTaxonomy};

const COMPUTE_ENGINE_SERVICE: &str = "6F81-5844-456A";

pub(super) async fn gcp_prices(observed_at: DateTime<Utc>) -> PriceSource {
    let mut source = PriceSource {
        provider: ProviderId::Gcp,
        state: PriceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        source: "GCP Cloud Billing Catalog API".to_string(),
        error: None,
        quotes: Vec::new(),
    };
    let auth = match crate::skarbiec::gcp_provider().await {
        Ok(auth) => auth,
        Err(error) => {
            source.state = PriceState::Blocked;
            source.error = Some(error.to_string());
            return source;
        }
    };
    let token = match auth
        .token(&["https://www.googleapis.com/auth/cloud-platform"])
        .await
    {
        Ok(token) => token,
        Err(error) => {
            source.state = PriceState::Blocked;
            source.error = Some(error.to_string());
            return source;
        }
    };
    let client = reqwest::Client::new();
    let taxonomy = match gcp_sku_taxonomy(&client, token.as_str()).await {
        Ok(taxonomy) => taxonomy,
        Err(error) => {
            source.state = PriceState::Blocked;
            source.error = Some(error);
            return source;
        }
    };
    let mut page_token: Option<String> = None;
    loop {
        let mut url = format!("https://cloudbilling.googleapis.com/v1/services/{COMPUTE_ENGINE_SERVICE}/skus?currencyCode=USD&pageSize=5000");
        if let Some(page) = page_token.as_deref() {
            url.push_str("&pageToken=");
            url.push_str(
                &url::form_urlencoded::byte_serialize(page.as_bytes()).collect::<String>(),
            );
        }
        let response = match client.get(&url).bearer_auth(token.as_str()).send().await {
            Ok(response) => response,
            Err(error) => {
                source.state = PriceState::Partial;
                source.error = Some(error.to_string());
                break;
            }
        };
        if !response.status().is_success() {
            source.state = PriceState::Partial;
            source.error = Some(format!(
                "Cloud Billing Catalog HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
            break;
        }
        let document: Value = match response.json().await {
            Ok(document) => document,
            Err(error) => {
                source.state = PriceState::Partial;
                source.error = Some(error.to_string());
                break;
            }
        };
        for sku in document
            .get("skus")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some((rate, unit)) = gcp_sku_hourly_rate(sku) else {
                continue;
            };
            let description = sku
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("GCP SKU")
                .to_string();
            // The SKU's own category says how it is bought: OnDemand and
            // Preemptible (Spot) are hourly prices a job can take; Commit1Yr,
            // Commit3Yr and every other usage type are commitments or
            // reservations and are not quoted. Sole-tenant nodes are their own
            // resource group.
            let category = sku.get("category");
            let usage_type = category
                .and_then(|category| category.get("usageType"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let purchase_option = match usage_type {
                "OnDemand" => "on_demand",
                "Preemptible" => "spot",
                _ => continue,
            };
            if category
                .and_then(|category| category.get("resourceGroup"))
                .and_then(Value::as_str)
                == Some("SoleTenancy")
            {
                continue;
            }
            // Custom-shape cores and memory share their family's taxonomy and
            // cost at least the predefined shape's rate, so the cheapest quote
            // of a family is the predefined one and they need no exclusion.
            let classified = sku
                .get("skuId")
                .and_then(Value::as_str)
                .and_then(|sku_id| taxonomy.get(sku_id));
            let regions: Vec<Option<String>> = sku
                .get("serviceRegions")
                .and_then(Value::as_array)
                .map(|regions| {
                    regions
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|region| Some(region.to_string()))
                        .collect()
                })
                .filter(|regions: &Vec<Option<String>>| !regions.is_empty())
                .unwrap_or_else(|| vec![None]);
            let family = match classified {
                Some(SkuTaxonomy::Family(family)) => Some(family.clone()),
                _ => None,
            };
            let accelerator = match classified {
                Some(SkuTaxonomy::Accelerator(accelerator)) => Some(accelerator.to_string()),
                _ => None,
            };
            for region in regions {
                source.quotes.push(PriceQuote {
                    provider: ProviderId::Gcp,
                    sku: sku
                        .get("skuId")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    description: description.clone(),
                    region,
                    machine_type: None,
                    accelerator_type: accelerator.clone(),
                    family: family.clone(),
                    purchase_option: purchase_option.to_string(),
                    unit: unit.to_string(),
                    hourly_usd: rate,
                    currency: "USD".to_string(),
                    source: "GCP Cloud Billing Catalog API".to_string(),
                    observed_at: observed_at.to_rfc3339(),
                    dynamic: true,
                });
            }
        }
        page_token = document
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if page_token.is_none() {
            break;
        }
    }
    if source.quotes.is_empty() && source.error.is_none() {
        source.state = PriceState::Partial;
        source.error = Some("Cloud Billing Catalog returned no hourly compute prices".to_string());
    }
    source
}

/// The SKU's hourly price and the unit it is priced in: `hour` for a price per
/// instance, core or accelerator hour, `gib_hour` for a price per GiB of
/// memory per hour. The unit comes from the SKU's `usageUnit`, which is how a
/// core price and a memory price of one machine family are told apart.
fn gcp_sku_hourly_rate(sku: &Value) -> Option<(f64, &'static str)> {
    let expression = sku.pointer("/pricingInfo/0/pricingExpression")?;
    let usage_unit = expression
        .get("usageUnit")
        .and_then(Value::as_str)
        .unwrap_or("");
    let unit = match usage_unit {
        "h" | "hour" => "hour",
        "GiBy.h" | "GBy.h" => "gib_hour",
        _ => return None,
    };
    let price = expression.pointer("/tieredRates/0/unitPrice")?;
    let units = price
        .get("units")
        .and_then(|value| {
            value
                .as_str()
                .and_then(|text| text.parse::<f64>().ok())
                .or_else(|| value.as_f64())
        })
        .unwrap_or_default();
    // `google.type.Money.nanos` counts billionths of a unit, the same scale
    // as std's nanoseconds of a second.
    let nanos = price
        .get("nanos")
        .and_then(Value::as_u64)
        .map(|nanos| std::time::Duration::from_nanos(nanos).as_secs_f64())
        .unwrap_or_default();
    Some((units + nanos, unit))
}
