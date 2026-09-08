//! The GCP price read: the Cloud Billing Catalog, paged, one quote per SKU
//! and service region.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::autonomy::model::SCHEMA_VERSION;
use crate::capabilities::ProviderId;

use super::{
    infer_accelerator, infer_machine_type, PriceQuote, PriceSource, PriceState,
    PRICING_HTTP_TIMEOUT,
};

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
    let client = reqwest::Client::builder()
        .timeout(PRICING_HTTP_TIMEOUT)
        .build()
        .expect("pricing HTTP client builds");
    let mut page_token: Option<String> = None;
    loop {
        let mut url = "https://cloudbilling.googleapis.com/v1/services/6F81-5844-456A/skus?currencyCode=USD&pageSize=5000".to_string();
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
            let Some(rate) = gcp_sku_hourly_rate(sku) else {
                continue;
            };
            let description = sku
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("GCP SKU")
                .to_string();
            let lowered_description = description.to_ascii_lowercase();
            if [
                "commitment",
                "committed use",
                "reservation",
                "sole tenancy",
                "custom instance",
            ]
            .iter()
            .any(|excluded| lowered_description.contains(excluded))
            {
                continue;
            }
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
            let accelerator = infer_accelerator(&description);
            let machine = infer_machine_type(&description);
            for region in regions {
                source.quotes.push(PriceQuote {
                    schema_version: SCHEMA_VERSION,
                    provider: ProviderId::Gcp,
                    sku: sku
                        .get("skuId")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    description: description.clone(),
                    region,
                    machine_type: machine.clone(),
                    accelerator_type: accelerator.clone(),
                    purchase_option: if description.to_ascii_lowercase().contains("spot")
                        || description.to_ascii_lowercase().contains("preemptible")
                    {
                        "spot".to_string()
                    } else {
                        "on_demand".to_string()
                    },
                    unit: "hour".to_string(),
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

fn gcp_sku_hourly_rate(sku: &Value) -> Option<f64> {
    let expression = sku.pointer("/pricingInfo/0/pricingExpression")?;
    let usage_unit = expression
        .get("usageUnit")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !matches!(usage_unit, "h" | "hour" | "GiBy.h" | "GBy.h") {
        return None;
    }
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
    let nanos = price
        .get("nanos")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let decimal_base = (u8::BITS + (u16::BITS / u8::BITS)) as f64;
    let nanos_exponent = (u8::BITS + true as u32) as i32;
    Some(units + nanos / decimal_base.powi(nanos_exponent))
}
