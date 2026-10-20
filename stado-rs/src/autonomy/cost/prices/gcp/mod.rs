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
    let mut left_out: Vec<String> = Vec::new();
    loop {
        let mut url = format!("https://cloudbilling.googleapis.com/v1/services/{COMPUTE_ENGINE_SERVICE}/skus?currencyCode=USD&pageSize=5000");
        if let Some(page) = page_token.as_deref() {
            url.push_str("&pageToken=");
            url.push_str(
                &url::form_urlencoded::byte_serialize(page.as_bytes()).collect::<String>(),
            );
        }
        let response =
            match crate::wait::request(client.get(&url).bearer_auth(token.as_str())).await {
                Ok(response) => response,
                Err(error) => {
                    source.state = PriceState::Partial;
                    source.error = Some(error.to_string());
                    break;
                }
            };
        let status = response.status();
        if !status.is_success() {
            source.state = PriceState::Partial;
            source.error = Some(format!(
                "Cloud Billing Catalog HTTP {status}: {}",
                crate::primitives::http::refused_body(response).await
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
            // The SKU's own category says how it is bought: OnDemand and
            // Preemptible (Spot) are hourly prices a job can take; Commit1Yr,
            // Commit3Yr and every other usage type, or none, are commitments
            // or reservations and are not quoted. Sole-tenant nodes are their
            // own resource group.
            let category = sku.get("category");
            let purchase_option = match category
                .and_then(|category| category.get("usageType"))
                .and_then(Value::as_str)
            {
                Some("OnDemand") => "on_demand",
                Some("Preemptible") => "spot",
                _ => continue,
            };
            if category
                .and_then(|category| category.get("resourceGroup"))
                .and_then(Value::as_str)
                == Some("SoleTenancy")
            {
                continue;
            }
            // A price without its SKU id or its description cannot be
            // classified, bought or shown, so it is left out and named in the
            // source's error.
            let (Some(sku_id), Some(description)) = (
                sku.get("skuId").and_then(Value::as_str),
                sku.get("description").and_then(Value::as_str),
            ) else {
                left_out.push(format!("a SKU without its skuId or description: {sku}"));
                continue;
            };
            let description = description.to_string();
            let (rate, unit) = match gcp_sku_hourly_rate(sku) {
                Ok(Some(rated)) => rated,
                Ok(None) => continue,
                Err(why) => {
                    left_out.push(format!("{sku_id} ({description}): {why}"));
                    continue;
                }
            };
            // Custom-shape cores and memory share their family's taxonomy and
            // cost at least the predefined shape's rate, so the cheapest quote
            // of a family is the predefined one and they need no exclusion.
            let classified = taxonomy.get(sku_id);
            let regions: Vec<Option<String>> =
                match sku.get("serviceRegions").and_then(Value::as_array) {
                    Some(listed) if !listed.is_empty() => listed
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|region| Some(region.to_string()))
                        .collect(),
                    // A SKU priced for no region is one price that holds in
                    // every region.
                    _ => vec![None],
                };
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
                    sku: sku_id.to_string(),
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
    if !left_out.is_empty() {
        let left_out = format!(
            "Cloud Billing Catalog prices left out: {} ({})",
            left_out.len(),
            left_out.join("; ")
        );
        source.state = PriceState::Partial;
        source.error = Some(match source.error.take() {
            Some(error) => format!("{error}; {left_out}"),
            None => left_out,
        });
    }
    if source.quotes.is_empty() && source.error.is_none() {
        source.state = PriceState::Partial;
        source.error = Some("Cloud Billing Catalog returned no hourly compute prices".to_string());
    }
    source
}

/// The SKU's hourly price and the unit it is priced in: `hour` for a price per
/// instance, core or accelerator hour, `gib_hour` for a price per GiB of
/// memory per hour; none for a SKU priced in any other unit, which is not an
/// hourly price. The unit comes from the SKU's `usageUnit`, which is how a
/// core price and a memory price of one machine family are told apart. A
/// price that cannot be read is refused with why.
fn gcp_sku_hourly_rate(sku: &Value) -> Result<Option<(f64, &'static str)>, String> {
    let Some(expression) = sku.pointer("/pricingInfo/0/pricingExpression") else {
        return Err("it carries no pricing expression".to_string());
    };
    let unit = match expression.get("usageUnit").and_then(Value::as_str) {
        Some("h" | "hour") => "hour",
        Some("GiBy.h" | "GBy.h") => "gib_hour",
        _ => return Ok(None),
    };
    let Some(price) = expression.pointer("/tieredRates/0/unitPrice") else {
        return Err("its pricing expression carries no unit price".to_string());
    };
    // `google.type.Money` writes `units` as an int64 string and `nanos` as
    // billionths of a unit, the same scale as std's nanoseconds of a second;
    // proto3 JSON leaves out a field that is zero, so either may be absent.
    let units = match price.get("units") {
        None => None,
        Some(Value::String(text)) => Some(
            text.parse::<f64>()
                .map_err(|error| format!("its units {text:?} are not a number: {error}"))?,
        ),
        Some(other) => return Err(format!("its units {other} are not an int64 string")),
    };
    let nanos = match price.get("nanos") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .map(|nanos| std::time::Duration::from_nanos(nanos).as_secs_f64())
                .ok_or_else(|| format!("its nanos {value} are not a count of billionths"))?,
        ),
    };
    let rate = match (units, nanos) {
        (Some(units), Some(nanos)) => units + nanos,
        (Some(units), None) => units,
        (None, Some(nanos)) => nanos,
        (None, None) => return Err(format!("its unit price {price} states no amount")),
    };
    Ok(Some((rate, unit)))
}
