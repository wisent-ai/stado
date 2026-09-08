//! The Azure price read: the Retail Prices API, filtered to consumption
//! Linux VM meters in the configured locations.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::autonomy::model::SCHEMA_VERSION;
use crate::capabilities::ProviderId;

use super::{infer_accelerator, PriceQuote, PriceSource, PriceState, PRICING_HTTP_TIMEOUT};

pub(super) async fn azure_prices(observed_at: DateTime<Utc>) -> PriceSource {
    let mut source = PriceSource {
        provider: ProviderId::Azure,
        state: PriceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        source: "Azure Retail Prices API".to_string(),
        error: None,
        quotes: Vec::new(),
    };
    let client = reqwest::Client::builder()
        .timeout(PRICING_HTTP_TIMEOUT)
        .build()
        .expect("pricing HTTP client builds");
    let regions = crate::config::azure_locations();
    let region_filter = regions
        .iter()
        .map(|region| format!("armRegionName eq '{region}'"))
        .collect::<Vec<_>>()
        .join(" or ");
    let mut filter = "serviceName eq 'Virtual Machines' and priceType eq 'Consumption'".to_string();
    if !region_filter.is_empty() {
        filter.push_str(" and (");
        filter.push_str(&region_filter);
        filter.push(')');
    }
    let mut endpoint = url::Url::parse("https://prices.azure.com/api/retail/prices")
        .expect("static Azure Retail Prices URL parses");
    endpoint.query_pairs_mut().append_pair("$filter", &filter);
    let mut url = endpoint.to_string();
    loop {
        let response = match client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                source.state = PriceState::Partial;
                source.error = Some(error.to_string());
                break;
            }
        };
        if !response.status().is_success() {
            source.state = PriceState::Partial;
            source.error = Some(format!("Azure Retail Prices HTTP {}", response.status()));
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
        for item in document
            .get("Items")
            .or_else(|| document.get("items"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let rate = item
                .get("retailPrice")
                .or_else(|| item.get("unitPrice"))
                .and_then(Value::as_f64)
                .unwrap_or_default();
            if rate <= f64::default() {
                continue;
            }
            let description = item
                .get("productName")
                .and_then(Value::as_str)
                .unwrap_or("Azure VM")
                .to_string();
            let price_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            let lowered_description = description.to_ascii_lowercase();
            if price_type != "Consumption"
                || lowered_description.contains("windows")
                || lowered_description.contains("reservation")
            {
                continue;
            }
            let meter = item.get("meterName").and_then(Value::as_str).unwrap_or("");
            source.quotes.push(PriceQuote {
                schema_version: SCHEMA_VERSION,
                provider: ProviderId::Azure,
                sku: item
                    .get("meterId")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                description: format!("{description} {meter}"),
                region: item
                    .get("armRegionName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                machine_type: item
                    .get("armSkuName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                accelerator_type: infer_accelerator(&format!("{description} {meter}")),
                purchase_option: if meter.to_ascii_lowercase().contains("spot")
                    || meter.to_ascii_lowercase().contains("low priority")
                    || item
                        .get("skuName")
                        .and_then(Value::as_str)
                        .is_some_and(|sku| sku.to_ascii_lowercase().contains("spot"))
                {
                    "spot".to_string()
                } else {
                    "on_demand".to_string()
                },
                unit: item
                    .get("unitOfMeasure")
                    .and_then(Value::as_str)
                    .unwrap_or("hour")
                    .to_string(),
                hourly_usd: rate,
                currency: item
                    .get("currencyCode")
                    .and_then(Value::as_str)
                    .unwrap_or("USD")
                    .to_string(),
                source: "Azure Retail Prices API".to_string(),
                observed_at: observed_at.to_rfc3339(),
                dynamic: true,
            });
        }
        let Some(next) = document
            .get("NextPageLink")
            .or_else(|| document.get("nextPageLink"))
            .and_then(Value::as_str)
            .filter(|next| !next.is_empty())
        else {
            break;
        };
        url = next.to_string();
    }
    if source.quotes.is_empty() && source.error.is_none() {
        source.state = PriceState::Partial;
        source.error = Some("Azure Retail Prices returned no VM prices".to_string());
    }
    source
}
