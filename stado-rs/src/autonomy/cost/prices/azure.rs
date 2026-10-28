//! The Azure price read: the Retail Prices API, filtered to consumption
//! Linux VM meters in the configured locations.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::capabilities::ProviderId;

use super::{infer_accelerator, PriceQuote, PriceSource, PriceState};

pub(super) async fn azure_prices(observed_at: DateTime<Utc>) -> PriceSource {
    let mut source = PriceSource {
        provider: ProviderId::Azure,
        state: PriceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        source: "Azure Retail Prices API".to_string(),
        error: None,
        quotes: Vec::new(),
    };
    let client = reqwest::Client::new();
    let regions = crate::config::azure_locations();
    if regions.is_empty() {
        // Reading every region's prices for a deployment that declared none
        // would price places Stado never dispatches to.
        source.state = PriceState::Blocked;
        source.error =
            Some("AZURE_LOCATIONS is not declared; no Azure region to price".to_string());
        return source;
    }
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
    let mut rows_without: Vec<String> = Vec::new();
    loop {
        let response = match crate::wait::request(client.get(&url)).await {
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
        // Azure Retail Prices always answers its page under Items, an empty
        // list included; a page without that list is a partial catalog, not
        // a page of no prices.
        let Some(items) = document
            .get("Items")
            .or_else(|| document.get("items"))
            .and_then(Value::as_array)
        else {
            source.state = PriceState::Partial;
            source.error = Some(format!(
                "Azure Retail Prices answered a page without Items: {url}"
            ));
            break;
        };
        for item in items {
            // A price row is only usable with what it prices, the kind of
            // price it is, the meter it bills, the unit it is quoted per and
            // its currency. One without them would be priced as meter
            // "unknown", per hour, in dollars, or as a product named "Azure
            // VM"; it is left out, and the source says how many rows it left
            // out and which fields they lacked.
            let rate = item.get("retailPrice").and_then(Value::as_f64);
            let product = item.get("productName").and_then(Value::as_str);
            let price_type = item.get("type").and_then(Value::as_str);
            let meter = item.get("meterName").and_then(Value::as_str);
            let meter_id = item.get("meterId").and_then(Value::as_str);
            let unit = item.get("unitOfMeasure").and_then(Value::as_str);
            let currency = item.get("currencyCode").and_then(Value::as_str);
            let (
                Some(rate),
                Some(product),
                Some(price_type),
                Some(meter),
                Some(meter_id),
                Some(unit),
                Some(currency),
            ) = (rate, product, price_type, meter, meter_id, unit, currency)
            else {
                let absent: Vec<&str> = [
                    ("retailPrice", rate.is_none()),
                    ("productName", product.is_none()),
                    ("type", price_type.is_none()),
                    ("meterName", meter.is_none()),
                    ("meterId", meter_id.is_none()),
                    ("unitOfMeasure", unit.is_none()),
                    ("currencyCode", currency.is_none()),
                ]
                .into_iter()
                .filter_map(|(field, missing)| missing.then_some(field))
                .collect();
                rows_without.push(absent.join("+"));
                continue;
            };
            // A free meter prices nothing, and only pay-as-you-go Linux
            // capacity is compared: reservations, dev/test and Windows
            // licences price something else.
            let priced = rate.is_normal() && rate.is_sign_positive();
            let windows = product.split(' ').any(|word| word == "Windows");
            if !priced || price_type != "Consumption" || windows {
                continue;
            }
            let description = product.to_string();
            source.quotes.push(PriceQuote {
                provider: ProviderId::Azure,
                sku: meter_id.to_string(),
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
                family: None,
                purchase_option: if spot_price(item.get("skuName").and_then(Value::as_str)) {
                    "spot".to_string()
                } else {
                    "on_demand".to_string()
                },
                unit: unit.to_string(),
                hourly_usd: rate,
                currency: currency.to_string(),
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
    if !rows_without.is_empty() {
        let kinds: std::collections::BTreeSet<&String> = rows_without.iter().collect();
        let counted: Vec<String> = kinds
            .into_iter()
            .map(|fields| {
                let count = rows_without.iter().filter(|row| *row == fields).count();
                format!("{count} without {fields}")
            })
            .collect();
        let left_out = format!("Azure Retail Prices rows left out: {}", counted.join(", "));
        source.state = PriceState::Partial;
        source.error = Some(match source.error.take() {
            Some(error) => format!("{error}; {left_out}"),
            None => left_out,
        });
    }
    if source.quotes.is_empty() && source.error.is_none() {
        source.state = PriceState::Partial;
        source.error = Some("Azure Retail Prices returned no VM prices".to_string());
    }
    source
}

/// Whether a retail price is a Spot (formerly Low Priority) price. The
/// Retail Prices API carries no priority field: Microsoft documents that a
/// Spot price's `skuName` carries the word `Spot`, its former Low Priority
/// prices the words `Low Priority`, and the API's values are case-sensitive
/// (https://learn.microsoft.com/en-us/azure/virtual-machines/spot-vms,
/// https://learn.microsoft.com/en-us/rest/api/cost-management/retail-prices/azure-retail-prices).
/// A word inside another word, or the same letters in another case, is not
/// that word.
fn spot_price(sku_name: Option<&str>) -> bool {
    sku_name.is_some_and(|sku| {
        sku.split(' ').any(|word| word == "Spot") || sku.ends_with(" Low Priority")
    })
}
