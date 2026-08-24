//! Dynamic pricing, cost allocation, forecast, anomalies, and savings ledger.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::ProviderId;
use crate::queue::{JobStorage, StorageError};

use super::model::{
    DecisionKind, InventorySnapshot, ResourceRecord, SavingsMeasurement, SavingsRecord,
    SCHEMA_VERSION,
};
use super::policy::AutonomyPolicy;

const HOURS_PER_DAY: f64 =
    (crate::monitor::billing::SECONDS_PER_DAY / crate::monitor::billing::SECONDS_PER_HOUR) as f64;
const BILLING_MONTH_DAYS: f64 =
    (u64::BITS / (u16::BITS / u8::BITS) - (u16::BITS / u8::BITS)) as f64;
const HOURS_PER_MONTH: f64 = HOURS_PER_DAY * BILLING_MONTH_DAYS;
const PRICING_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(
    crate::monitor::billing::SECONDS_PER_MINUTE / (u16::BITS / u8::BITS) as u64,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceState {
    Complete,
    Partial,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceQuote {
    pub schema_version: u16,
    pub provider: ProviderId,
    pub sku: String,
    pub description: String,
    pub region: Option<String>,
    pub machine_type: Option<String>,
    pub accelerator_type: Option<String>,
    pub purchase_option: String,
    pub unit: String,
    pub hourly_usd: f64,
    pub currency: String,
    pub source: String,
    pub observed_at: String,
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceSource {
    pub provider: ProviderId,
    pub state: PriceState,
    pub observed_at: String,
    pub source: String,
    pub error: Option<String>,
    pub quotes: Vec<PriceQuote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceBook {
    pub schema_version: u16,
    pub created_at: String,
    pub sources: Vec<PriceSource>,
    pub quotes: Vec<PriceQuote>,
}

impl PriceBook {
    pub fn find_hourly(
        &self,
        provider: ProviderId,
        region: Option<&str>,
        machine_type: &str,
        accelerator_type: &str,
        preemptible: bool,
    ) -> Option<PriceQuote> {
        let purchase = if preemptible { "spot" } else { "on_demand" };
        let matching = |quote: &&PriceQuote| {
            quote.provider == provider
                && quote.hourly_usd > f64::default()
                && quote.purchase_option == purchase
                && region.is_none_or(|wanted| {
                    quote
                        .region
                        .as_deref()
                        .is_none_or(|actual| actual == wanted || actual == "global")
                })
        };
        if let Some(exact) = self
            .quotes
            .iter()
            .filter(matching)
            .filter(|quote| {
                quote.machine_type.as_deref() == Some(machine_type)
                    || (!machine_type.is_empty()
                        && quote
                            .description
                            .to_ascii_lowercase()
                            .contains(&machine_type.to_ascii_lowercase()))
            })
            .min_by(|left, right| {
                left.hourly_usd
                    .partial_cmp(&right.hourly_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        {
            return Some(exact.clone());
        }
        if provider == ProviderId::Gcp {
            return self.gcp_composite_hourly(region, machine_type, accelerator_type, purchase);
        }
        self.quotes
            .iter()
            .filter(matching)
            .filter(|quote| {
                quote.accelerator_type.as_deref() == Some(accelerator_type)
                    || (!accelerator_type.is_empty()
                        && normalized(&quote.description).contains(&normalized(accelerator_type)))
            })
            .min_by(|left, right| {
                left.hourly_usd
                    .partial_cmp(&right.hourly_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    fn gcp_composite_hourly(
        &self,
        region: Option<&str>,
        machine_type: &str,
        accelerator_type: &str,
        purchase: &str,
    ) -> Option<PriceQuote> {
        let (family, cores, memory_gb, accelerator_count) = gcp_machine_shape(machine_type)?;
        let family_key = normalized(family);
        let core = self.cheapest_gcp_quote(region, purchase, |quote| {
            let description = normalized(&quote.description);
            quote.machine_type.is_none()
                && quote.accelerator_type.is_none()
                && description.contains(&family_key)
                && description.contains("core")
        })?;
        let memory = self.cheapest_gcp_quote(region, purchase, |quote| {
            let description = normalized(&quote.description);
            quote.machine_type.is_none()
                && quote.accelerator_type.is_none()
                && description.contains(&family_key)
                && (description.contains("ram") || description.contains("memory"))
        })?;
        let accelerator = self.cheapest_gcp_quote(region, purchase, |quote| {
            quote.accelerator_type.as_deref() == Some(accelerator_type)
                || normalized(&quote.description).contains(&normalized(accelerator_type))
        })?;
        Some(PriceQuote {
            schema_version: SCHEMA_VERSION,
            provider: ProviderId::Gcp,
            sku: format!("{}+{}+{}", core.sku, memory.sku, accelerator.sku),
            description: format!(
                "GCP {machine_type} + {accelerator_type} composed from live Billing SKUs"
            ),
            region: region.map(str::to_string),
            machine_type: Some(machine_type.to_string()),
            accelerator_type: Some(accelerator_type.to_string()),
            purchase_option: purchase.to_string(),
            unit: "hour".to_string(),
            hourly_usd: core.hourly_usd * cores
                + memory.hourly_usd * memory_gb
                + accelerator.hourly_usd * accelerator_count,
            currency: "USD".to_string(),
            source: "GCP Cloud Billing Catalog API (composed)".to_string(),
            observed_at: [core, memory, accelerator]
                .iter()
                .map(|quote| quote.observed_at.as_str())
                .max()
                .unwrap_or_default()
                .to_string(),
            dynamic: true,
        })
    }

    fn cheapest_gcp_quote(
        &self,
        region: Option<&str>,
        purchase: &str,
        predicate: impl Fn(&PriceQuote) -> bool,
    ) -> Option<&PriceQuote> {
        self.quotes
            .iter()
            .filter(|quote| {
                quote.provider == ProviderId::Gcp
                    && quote.purchase_option == purchase
                    && quote.hourly_usd > f64::default()
                    && region.is_none_or(|wanted| {
                        quote
                            .region
                            .as_deref()
                            .is_none_or(|actual| actual == wanted || actual == "global")
                    })
            })
            .filter(|quote| predicate(quote))
            .min_by(|left, right| {
                left.hourly_usd
                    .partial_cmp(&right.hourly_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

fn gcp_machine_shape(machine_type: &str) -> Option<(&'static str, f64, f64, f64)> {
    let parse = |value: &str| value.parse::<f64>().ok();
    if let Some(raw_cores) = machine_type.strip_prefix("n1-standard-") {
        let cores = parse(raw_cores)?;
        return Some(("n1", cores, cores * parse("3.75")?, parse("1")?));
    }
    if let Some(raw_cores) = machine_type.strip_prefix("g2-standard-") {
        let cores = parse(raw_cores)?;
        return Some(("g2", cores, cores * parse("4")?, parse("1")?));
    }
    match machine_type {
        "a2-highgpu-1g" => Some(("a2", parse("12")?, parse("85")?, parse("1")?)),
        "a2-ultragpu-1g" => Some(("a2", parse("12")?, parse("170")?, parse("1")?)),
        "a3-highgpu-1g" => Some(("a3", parse("26")?, parse("234")?, parse("1")?)),
        "a3-ultragpu-8g" => Some(("a3", parse("224")?, parse("1872")?, parse("8")?)),
        _ => None,
    }
}

pub async fn refresh_prices(policy: &AutonomyPolicy) -> PriceBook {
    let observed_at = Utc::now();
    let configured: std::collections::BTreeSet<ProviderId> = crate::config::wc_providers()
        .iter()
        .filter_map(|name| crate::capabilities::provider(name))
        .collect();
    let (gcp, azure, aws) = tokio::join!(
        async {
            if configured.contains(&ProviderId::Gcp) {
                Some(gcp_prices(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Azure) {
                Some(azure_prices(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Aws) {
                Some(aws_spot_prices(observed_at).await)
            } else {
                None
            }
        },
    );
    let mut sources: Vec<PriceSource> = [gcp, azure, aws].into_iter().flatten().collect();
    if let Some(hourly) = policy.local_hourly_cost_usd {
        sources.push(PriceSource {
            provider: ProviderId::Local,
            state: PriceState::Complete,
            observed_at: observed_at.to_rfc3339(),
            source: "autonomy policy".to_string(),
            error: None,
            quotes: vec![PriceQuote {
                schema_version: SCHEMA_VERSION,
                provider: ProviderId::Local,
                sku: "local-capacity".to_string(),
                description: "Configured marginal local host cost".to_string(),
                region: None,
                machine_type: None,
                accelerator_type: None,
                purchase_option: "on_demand".to_string(),
                unit: "hour".to_string(),
                hourly_usd: hourly,
                currency: "USD".to_string(),
                source: "autonomy policy".to_string(),
                observed_at: observed_at.to_rfc3339(),
                dynamic: true,
            }],
        });
    }
    let quotes = sources
        .iter()
        .flat_map(|source| source.quotes.iter().cloned())
        .collect();
    PriceBook {
        schema_version: SCHEMA_VERSION,
        created_at: observed_at.to_rfc3339(),
        sources,
        quotes,
    }
}

async fn gcp_prices(observed_at: DateTime<Utc>) -> PriceSource {
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

async fn azure_prices(observed_at: DateTime<Utc>) -> PriceSource {
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

async fn aws_spot_prices(observed_at: DateTime<Utc>) -> PriceSource {
    let mut source = PriceSource {
        provider: ProviderId::Aws,
        state: PriceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        source: "EC2 Spot Price History + AWS Price List".to_string(),
        error: None,
        quotes: Vec::new(),
    };
    let region = crate::config::aws_region();
    let sdk = match crate::providers::aws::sdk_config(region).await {
        Ok(sdk) => sdk,
        Err(error) => {
            source.state = PriceState::Blocked;
            source.error = Some(error.to_string());
            return source;
        }
    };
    let instance_names: Vec<&str> = crate::catalog::AWS_INSTANCE_TO_ACCEL
        .keys()
        .copied()
        .collect();
    let client = aws_sdk_ec2::Client::new(&sdk);
    let instance_types = instance_names
        .iter()
        .map(|machine| aws_sdk_ec2::types::InstanceType::from(*machine))
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    match client
        .describe_spot_price_history()
        .set_instance_types(Some(instance_types))
        .product_descriptions("Linux/UNIX")
        .send()
        .await
    {
        Ok(output) => {
            let mut rates = BTreeMap::<String, Vec<f64>>::new();
            for item in output.spot_price_history() {
                let Some(machine) = item.instance_type().map(|kind| kind.as_str().to_string())
                else {
                    continue;
                };
                let Some(rate) = item
                    .spot_price()
                    .and_then(|value| value.parse::<f64>().ok())
                else {
                    continue;
                };
                rates.entry(machine).or_default().push(rate);
            }
            let rates = rates.into_iter().filter_map(|(machine, mut samples)| {
                samples.sort_by(|left, right| {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                });
                let divisor = (u16::BITS / u8::BITS) as usize;
                samples
                    .get(samples.len() / divisor)
                    .copied()
                    .map(|rate| (machine, rate))
            });
            for (machine, rate) in rates {
                source.quotes.push(aws_quote(
                    &machine,
                    "spot",
                    rate,
                    region,
                    "EC2 DescribeSpotPriceHistory",
                    observed_at,
                ));
            }
        }
        Err(error) => failures.push(format!("spot: {error}")),
    }
    match aws_on_demand_prices(&sdk, region, observed_at, &instance_names).await {
        Ok(quotes) => source.quotes.extend(quotes),
        Err(error) => failures.push(format!("on-demand: {error}")),
    }
    if !failures.is_empty() {
        source.state = if source.quotes.is_empty() {
            PriceState::Blocked
        } else {
            PriceState::Partial
        };
        source.error = Some(failures.join("; "));
    }
    source
}

async fn aws_on_demand_prices(
    sdk: &aws_config::SdkConfig,
    region: &str,
    observed_at: DateTime<Utc>,
    instance_names: &[&str],
) -> Result<Vec<PriceQuote>, String> {
    use aws_sdk_pricing::types::{Filter, FilterType};

    let pricing_config = aws_sdk_pricing::config::Builder::from(sdk)
        .region(aws_config::Region::new("us-east-1"))
        .build();
    let client = aws_sdk_pricing::Client::from_conf(pricing_config);
    let filter = |field: &str, value: &str, kind: FilterType| {
        Filter::builder()
            .field(field)
            .value(value)
            .r#type(kind)
            .build()
            .map_err(|error| error.to_string())
    };
    let filters = vec![
        filter("instanceType", &instance_names.join(","), FilterType::AnyOf)?,
        filter("regionCode", region, FilterType::TermMatch)?,
        filter("operatingSystem", "Linux", FilterType::TermMatch)?,
        filter("tenancy", "Shared", FilterType::TermMatch)?,
        filter("preInstalledSw", "NA", FilterType::TermMatch)?,
        filter("capacitystatus", "Used", FilterType::TermMatch)?,
    ];
    let mut token = None;
    let mut rates = BTreeMap::<String, f64>::new();
    loop {
        let output = client
            .get_products()
            .service_code("AmazonEC2")
            .set_filters(Some(filters.clone()))
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        for raw in output.price_list() {
            let Ok(value) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            let Some(machine) = value
                .pointer("/product/attributes/instanceType")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(terms) = value.pointer("/terms/OnDemand").and_then(Value::as_object) else {
                continue;
            };
            for term in terms.values().filter_map(Value::as_object) {
                let Some(dimensions) = term.get("priceDimensions").and_then(Value::as_object)
                else {
                    continue;
                };
                for dimension in dimensions.values() {
                    if dimension.get("unit").and_then(Value::as_str) != Some("Hrs") {
                        continue;
                    }
                    let Some(rate) = dimension
                        .pointer("/pricePerUnit/USD")
                        .and_then(Value::as_str)
                        .and_then(|raw| raw.parse::<f64>().ok())
                    else {
                        continue;
                    };
                    rates
                        .entry(machine.to_string())
                        .and_modify(|current| *current = current.min(rate))
                        .or_insert(rate);
                }
            }
        }
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    Ok(rates
        .into_iter()
        .map(|(machine, rate)| {
            aws_quote(
                &machine,
                "on_demand",
                rate,
                region,
                "AWS Price List GetProducts",
                observed_at,
            )
        })
        .collect())
}

fn aws_quote(
    machine: &str,
    purchase_option: &str,
    hourly_usd: f64,
    region: &str,
    source: &str,
    observed_at: DateTime<Utc>,
) -> PriceQuote {
    PriceQuote {
        schema_version: SCHEMA_VERSION,
        provider: ProviderId::Aws,
        sku: machine.to_string(),
        description: format!("AWS EC2 {machine} {purchase_option}"),
        region: Some(region.to_string()),
        machine_type: Some(machine.to_string()),
        accelerator_type: crate::catalog::AWS_INSTANCE_TO_ACCEL
            .get(machine)
            .map(|accelerator| accelerator.to_string()),
        purchase_option: purchase_option.to_string(),
        unit: "hour".to_string(),
        hourly_usd,
        currency: "USD".to_string(),
        source: source.to_string(),
        observed_at: observed_at.to_rfc3339(),
        dynamic: true,
    }
}

pub fn enrich_inventory(snapshot: &mut InventorySnapshot, prices: &PriceBook) {
    for resource in &mut snapshot.resources {
        enrich_resource(resource, prices);
    }
    for source in &mut snapshot.sources {
        for resource in &mut source.resources {
            enrich_resource(resource, prices);
        }
    }
}

fn enrich_resource(resource: &mut ResourceRecord, prices: &PriceBook) {
    let machine_type = resource
        .evidence
        .get("machine_type")
        .or_else(|| resource.evidence.get("instance_type"))
        .or_else(|| resource.evidence.pointer("/sku/name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let accelerator = resource
        .evidence
        .get("accelerator_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let preemptible = resource
        .evidence
        .get("preemptible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(quote) = prices.find_hourly(
        resource.provider,
        resource.region.as_deref(),
        machine_type,
        accelerator,
        preemptible,
    ) {
        resource.current_hourly_cost_usd = Some(quote.hourly_usd);
        resource.forecast_monthly_cost_usd = Some(quote.hourly_usd * HOURS_PER_MONTH);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
    pub schema_version: u16,
    pub entry_id: String,
    pub provider: ProviderId,
    pub service: String,
    pub resource_id: Option<String>,
    pub job_id: Option<String>,
    pub workload: Option<String>,
    pub owner: Option<String>,
    pub environment: Option<String>,
    pub region: Option<String>,
    pub usage_started_at: Option<String>,
    pub usage_ended_at: Option<String>,
    pub gross_cost_usd: f64,
    pub credits_usd: f64,
    pub net_cost_usd: f64,
    pub source: String,
    pub allocated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBucket {
    pub gross_cost_usd: f64,
    pub credits_usd: f64,
    pub net_cost_usd: f64,
    pub entries: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationReport {
    pub schema_version: u16,
    pub created_at: String,
    pub entries: Vec<CostEntry>,
    pub by_provider: BTreeMap<String, CostBucket>,
    pub by_owner: BTreeMap<String, CostBucket>,
    pub by_workload: BTreeMap<String, CostBucket>,
    pub allocated: CostBucket,
    pub unallocated: CostBucket,
}

pub async fn build_allocation(
    _store: &JobStorage,
    inventory: &InventorySnapshot,
) -> Result<AllocationReport, StorageError> {
    let mut entries = Vec::new();
    for resource in &inventory.resources {
        if resource.resource_type == "instance"
            && matches!(
                resource.state.to_ascii_lowercase().as_str(),
                "stopped" | "stopping" | "terminated" | "deallocated" | "deallocating"
            )
        {
            continue;
        }
        let Some(hourly) = resource.current_hourly_cost_usd else {
            continue;
        };
        entries.push(resource_cost_entry(resource, hourly));
    }
    Ok(aggregate_allocation(entries))
}

fn resource_cost_entry(resource: &ResourceRecord, hourly: f64) -> CostEntry {
    CostEntry {
        schema_version: SCHEMA_VERSION,
        entry_id: format!("resource:{}", resource.resource_id),
        provider: resource.provider,
        service: resource.resource_type.clone(),
        resource_id: Some(resource.resource_id.clone()),
        job_id: None,
        workload: resource.workload.clone(),
        owner: resource.owner.clone(),
        environment: resource.environment.clone(),
        region: resource.region.clone(),
        usage_started_at: None,
        usage_ended_at: None,
        gross_cost_usd: hourly,
        credits_usd: f64::default(),
        net_cost_usd: hourly,
        source: "live hourly price".to_string(),
        allocated: resource.owner.is_some() || resource.workload.is_some(),
    }
}

fn aggregate_allocation(entries: Vec<CostEntry>) -> AllocationReport {
    let mut report = AllocationReport {
        schema_version: SCHEMA_VERSION,
        created_at: Utc::now().to_rfc3339(),
        entries,
        by_provider: BTreeMap::new(),
        by_owner: BTreeMap::new(),
        by_workload: BTreeMap::new(),
        allocated: CostBucket::default(),
        unallocated: CostBucket::default(),
    };
    for entry in &report.entries {
        add_bucket(
            report
                .by_provider
                .entry(entry.provider.as_str().to_string())
                .or_default(),
            entry,
        );
        add_bucket(
            report
                .by_owner
                .entry(
                    entry
                        .owner
                        .clone()
                        .unwrap_or_else(|| "unallocated".to_string()),
                )
                .or_default(),
            entry,
        );
        add_bucket(
            report
                .by_workload
                .entry(
                    entry
                        .workload
                        .clone()
                        .unwrap_or_else(|| "unallocated".to_string()),
                )
                .or_default(),
            entry,
        );
        if entry.allocated {
            add_bucket(&mut report.allocated, entry);
        } else {
            add_bucket(&mut report.unallocated, entry);
        }
    }
    report
}

fn add_bucket(bucket: &mut CostBucket, entry: &CostEntry) {
    bucket.gross_cost_usd += entry.gross_cost_usd;
    bucket.credits_usd += entry.credits_usd;
    bucket.net_cost_usd += entry.net_cost_usd;
    bucket.entries += true as usize;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostForecast {
    pub schema_version: u16,
    pub created_at: String,
    pub current_hourly_usd: f64,
    pub end_of_day_usd: f64,
    pub end_of_month_usd: f64,
    pub hourly_budget_usd: Option<f64>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub hourly_overrun_usd: f64,
    pub daily_overrun_usd: f64,
    pub budget_exceeded: bool,
    pub projected_overrun_usd: f64,
    pub credit_runway_days: Option<f64>,
}

pub fn forecast(
    allocation: &AllocationReport,
    policy: &AutonomyPolicy,
    billing_snapshot: Option<&Value>,
    now: DateTime<Utc>,
) -> CostForecast {
    let mut current_hourly = allocation
        .entries
        .iter()
        .filter(|entry| entry.source == "live hourly price")
        .map(|entry| entry.net_cost_usd)
        .sum::<f64>();
    let elapsed_hours = now.hour() as f64;
    let month_days = days_in_month(now) as f64;
    let elapsed_month_hours = ((now.day() - true as u32) as f64 * HOURS_PER_DAY) + elapsed_hours;
    let remaining_month_hours =
        (month_days * HOURS_PER_DAY - elapsed_month_hours).max(f64::default());
    let spent = billing_net_cost(billing_snapshot).unwrap_or_default();
    if elapsed_month_hours > f64::default() {
        current_hourly = current_hourly.max(spent / elapsed_month_hours);
    }
    let end_of_month = spent + current_hourly * remaining_month_hours;
    let end_of_day = current_hourly * HOURS_PER_DAY;
    let hourly_overrun = policy
        .budgets
        .hourly_usd
        .map(|limit| (current_hourly - limit).max(f64::default()))
        .unwrap_or_default();
    let daily_overrun = policy
        .budgets
        .daily_usd
        .map(|limit| (end_of_day - limit).max(f64::default()))
        .unwrap_or_default();
    let budget = policy.budgets.monthly_usd;
    CostForecast {
        schema_version: SCHEMA_VERSION,
        created_at: now.to_rfc3339(),
        current_hourly_usd: current_hourly,
        end_of_day_usd: end_of_day,
        end_of_month_usd: end_of_month,
        hourly_budget_usd: policy.budgets.hourly_usd,
        daily_budget_usd: policy.budgets.daily_usd,
        monthly_budget_usd: budget,
        projected_overrun_usd: budget
            .map(|limit| (end_of_month - limit).max(f64::default()))
            .unwrap_or_default(),
        hourly_overrun_usd: hourly_overrun,
        daily_overrun_usd: daily_overrun,
        budget_exceeded: hourly_overrun > f64::default()
            || daily_overrun > f64::default()
            || budget.is_some_and(|limit| end_of_month > limit),
        credit_runway_days: credit_balance(billing_snapshot).and_then(|balance| {
            let daily = current_hourly * HOURS_PER_DAY;
            (daily > f64::default()).then_some(balance / daily)
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostAnomaly {
    pub anomaly_id: String,
    pub severity: String,
    pub kind: String,
    pub subject: String,
    pub reason: String,
    pub observed_value: f64,
    pub expected_value: f64,
}

pub fn detect_anomalies(
    allocation: &AllocationReport,
    inventory: &InventorySnapshot,
    forecast: &CostForecast,
) -> Vec<CostAnomaly> {
    let mut anomalies = Vec::new();
    if forecast.hourly_overrun_usd > f64::default() {
        anomalies.push(anomaly(
            "hourly-budget-overrun",
            "critical",
            "budget_forecast",
            "hourly-budget",
            "current hourly burn exceeds the configured budget",
            forecast.current_hourly_usd,
            forecast.hourly_budget_usd.unwrap_or_default(),
        ));
    }
    if forecast.daily_overrun_usd > f64::default() {
        anomalies.push(anomaly(
            "daily-budget-overrun",
            "critical",
            "budget_forecast",
            "daily-budget",
            "projected daily run-rate exceeds the configured budget",
            forecast.end_of_day_usd,
            forecast.daily_budget_usd.unwrap_or_default(),
        ));
    }
    if forecast.projected_overrun_usd > f64::default() {
        anomalies.push(anomaly(
            "budget-overrun",
            "critical",
            "budget_forecast",
            "monthly-budget",
            "projected month-end cost exceeds the configured budget",
            forecast.end_of_month_usd,
            forecast.monthly_budget_usd.unwrap_or_default(),
        ));
    }
    if allocation.unallocated.net_cost_usd > f64::default() {
        anomalies.push(anomaly(
            "unallocated-spend",
            "high",
            "unallocated_cost",
            "cost-ledger",
            "cost exists without an owner or workload attribution",
            allocation.unallocated.net_cost_usd,
            f64::default(),
        ));
    }
    for resource in &inventory.resources {
        let hourly = resource.current_hourly_cost_usd.unwrap_or_default();
        if hourly <= f64::default() {
            continue;
        }
        let utilization = resource
            .utilization
            .get("gpu")
            .or_else(|| resource.utilization.get("cpu"))
            .copied();
        if utilization.is_some_and(|value| value <= f64::EPSILON) {
            anomalies.push(anomaly(
                &format!("idle:{}", resource.resource_id),
                "high",
                "idle_paid_resource",
                &resource.resource_id,
                "paid resource reports no utilization",
                hourly,
                f64::default(),
            ));
        }
        if resource.ownership == super::model::Ownership::Unknown {
            anomalies.push(anomaly(
                &format!("unknown-owner:{}", resource.resource_id),
                "medium",
                "unknown_owner_spend",
                &resource.resource_id,
                "paid resource has no Stado ownership contract",
                hourly,
                f64::default(),
            ));
        }
    }
    for (provider, bucket) in &allocation.by_provider {
        let attributed_resources = allocation
            .entries
            .iter()
            .filter(|entry| {
                entry.provider.as_str() == provider
                    && (entry.workload.is_some() || entry.job_id.is_some())
            })
            .count();
        if bucket.net_cost_usd > f64::default() && attributed_resources == usize::default() {
            anomalies.push(anomaly(
                &format!("provider-without-workload:{provider}"),
                "medium",
                "provider_spend_without_workload",
                provider,
                "provider cost exists without workload attribution",
                bucket.net_cost_usd,
                f64::default(),
            ));
        }
    }
    anomalies
}

fn anomaly(
    id: &str,
    severity: &str,
    kind: &str,
    subject: &str,
    reason: &str,
    observed: f64,
    expected: f64,
) -> CostAnomaly {
    CostAnomaly {
        anomaly_id: id.to_string(),
        severity: severity.to_string(),
        kind: kind.to_string(),
        subject: subject.to_string(),
        reason: reason.to_string(),
        observed_value: observed,
        expected_value: expected,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutcomeSummary {
    pub feedback_written: usize,
    pub savings_measured: usize,
}
pub async fn measure_outcomes(store: &JobStorage) -> Result<OutcomeSummary, StorageError> {
    let decisions = super::storage::list_decisions(store).await?;
    let feedback = super::storage::list_feedback(store).await?;
    let feedback_ids: std::collections::BTreeSet<&str> = feedback
        .iter()
        .map(|entry| entry.decision_id.as_str())
        .collect();
    let savings = super::storage::list_savings(store).await?;
    let measurements = super::storage::list_savings_measurements(store).await?;
    let measured_ids: std::collections::BTreeSet<&str> = measurements
        .iter()
        .map(|entry| entry.savings_id.as_str())
        .collect();
    let costs = crate::scheduler::cost::collect_completed_dynamic(store).await?;
    let mut summary = OutcomeSummary::default();
    for decision in decisions
        .iter()
        .filter(|decision| decision.kind == DecisionKind::Placement)
    {
        let completed = store.read_job("completed", &decision.subject_id).await?;
        let failed = if completed.is_none() {
            store.read_job("failed", &decision.subject_id).await?
        } else {
            None
        };
        let Some(job) = completed.as_ref().or(failed.as_ref()) else {
            continue;
        };
        let target_id = decision
            .selected
            .as_ref()
            .and_then(|selected| selected.get("target_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if !feedback_ids.contains(decision.decision_id.as_str()) {
            let feedback = super::storage::PlacementFeedback {
                schema_version: SCHEMA_VERSION,
                decision_id: decision.decision_id.clone(),
                subject_id: decision.subject_id.clone(),
                target_id,
                observed_at: Utc::now().to_rfc3339(),
                startup_seconds: elapsed(Some(job.created_at.as_str()), job.started_at.as_deref()),
                runtime_seconds: elapsed(
                    job.started_at.as_deref(),
                    job.completed_at.as_deref().or(job.failed_at.as_deref()),
                ),
                realized_cost_usd: costs
                    .iter()
                    .find(|row| row.job_id == decision.subject_id)
                    .map(|row| row.cost_usd),
                succeeded: completed.is_some(),
                failure_class: failed
                    .as_ref()
                    .and_then(|job| job.error.as_deref())
                    .map(str::to_string),
            };
            match super::storage::write_feedback(store, &feedback).await {
                Ok(()) => summary.feedback_written += true as usize,
                Err(StorageError::StorageConflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let Some(cost) = costs
            .iter()
            .find(|row| row.job_id == decision.subject_id)
            .map(|row| row.cost_usd)
        else {
            continue;
        };
        for saving in savings
            .iter()
            .filter(|saving| saving.decision_id == decision.decision_id)
            .filter(|saving| !measured_ids.contains(saving.savings_id.as_str()))
        {
            let measurement = SavingsMeasurement {
                schema_version: SCHEMA_VERSION,
                measurement_id: format!("measurement-{}", saving.savings_id),
                savings_id: saving.savings_id.clone(),
                decision_id: saving.decision_id.clone(),
                measured_at: Utc::now().to_rfc3339(),
                realized_cost_usd: cost,
                realized_savings_usd: saving.baseline_cost_usd - cost,
                source: "completed job cost attribution".to_string(),
                source_invoice_period: None,
            };
            match super::storage::write_savings_measurement(store, &measurement).await {
                Ok(()) => summary.savings_measured += true as usize,
                Err(StorageError::StorageConflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(summary)
}

fn elapsed(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    let start = DateTime::parse_from_rfc3339(start?).ok()?;
    let end = DateTime::parse_from_rfc3339(end?).ok()?;
    let milliseconds_per_second = chrono::Duration::seconds(true as i64).num_milliseconds() as f64;
    Some(end.signed_duration_since(start).num_milliseconds() as f64 / milliseconds_per_second)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavingsSummary {
    pub records: usize,
    pub predicted_savings_usd: f64,
    pub realized_savings_usd: f64,
    pub pending_measurement: usize,
    pub by_provider: BTreeMap<String, f64>,
}

pub fn summarize_savings(records: &[SavingsRecord]) -> SavingsSummary {
    summarize_savings_with_measurements(records, &[])
}

pub fn summarize_savings_with_measurements(
    records: &[SavingsRecord],
    measurements: &[SavingsMeasurement],
) -> SavingsSummary {
    let by_savings: BTreeMap<&str, &SavingsMeasurement> = measurements
        .iter()
        .map(|measurement| (measurement.savings_id.as_str(), measurement))
        .collect();
    let mut summary = SavingsSummary {
        records: records.len(),
        ..SavingsSummary::default()
    };
    for record in records {
        summary.predicted_savings_usd += record.predicted_savings_usd;
        if let Some(measurement) = by_savings.get(record.savings_id.as_str()) {
            summary.realized_savings_usd += measurement.realized_savings_usd;
            *summary
                .by_provider
                .entry(record.provider.as_str().to_string())
                .or_default() += measurement.realized_savings_usd;
        } else {
            summary.pending_measurement += true as usize;
        }
    }
    summary
}

pub async fn persist_reports(
    store: &JobStorage,
    prices: &PriceBook,
    allocation: &AllocationReport,
    forecast: &CostForecast,
    anomalies: &[CostAnomaly],
) -> Result<(), StorageError> {
    for (path, value) in [
        (
            "state/autonomy/cost/prices.json",
            serde_json::to_value(prices)?,
        ),
        (
            "state/autonomy/cost/allocation.json",
            serde_json::to_value(allocation)?,
        ),
        (
            "state/autonomy/cost/forecast.json",
            serde_json::to_value(forecast)?,
        ),
        (
            "state/autonomy/cost/anomalies.json",
            serde_json::to_value(anomalies)?,
        ),
    ] {
        store
            .upload_text(path, &serde_json::to_string(&value)?)
            .await?;
    }
    Ok(())
}

pub async fn load_billing_snapshot(store: &JobStorage) -> Result<Option<Value>, StorageError> {
    crate::monitor::billing::load_snapshot(store).await
}

fn billing_net_cost(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .pointer("/gcp/latest_month/net_cost")
        .or_else(|| snapshot.pointer("/gcp/net_cost"))
        .and_then(Value::as_f64)
}

fn credit_balance(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .pointer("/azure/available_balance")
        .or_else(|| snapshot.pointer("/azure/balance"))
        .and_then(Value::as_f64)
}

fn infer_accelerator(description: &str) -> Option<String> {
    let normalized_description = normalized(description);
    [
        ("teslak80", "nvidia-tesla-k80"),
        ("teslap100", "nvidia-tesla-p100"),
        ("teslap40", "nvidia-tesla-p40"),
        ("teslat4", "nvidia-tesla-t4"),
        ("teslav100", "nvidia-tesla-v100"),
        ("a10080gb", "nvidia-a100-80gb"),
        ("teslaa100", "nvidia-tesla-a100"),
        ("a100", "nvidia-tesla-a100"),
        ("h10094gb", "nvidia-h100-94gb"),
        ("h10080gb", "nvidia-h100-80gb"),
        ("h100", "nvidia-h100-80gb"),
        ("h200", "nvidia-h200-141gb"),
        ("gb200", "nvidia-gb200-192gb"),
        ("b200", "nvidia-b200-180gb"),
        ("l4", "nvidia-l4"),
        ("a10", "nvidia-a10"),
        ("mi300x", "amd-mi300x-192gb"),
    ]
    .iter()
    .find(|(alias, _)| normalized_description.contains(alias))
    .map(|(_, canonical)| (*canonical).to_string())
}

fn infer_machine_type(description: &str) -> Option<String> {
    description
        .split_whitespace()
        .find(|word| {
            let lowered = word.to_ascii_lowercase();
            lowered.starts_with("a2-")
                || lowered.starts_with("g2-")
                || lowered.starts_with("n1-")
                || lowered.starts_with("standard_")
                || lowered.starts_with("p3.")
                || lowered.starts_with("p4.")
                || lowered.starts_with("g4dn.")
                || lowered.starts_with("g5.")
        })
        .map(|word| {
            word.trim_matches(|character: char| {
                !character.is_alphanumeric()
                    && character != '-'
                    && character != '_'
                    && character != '.'
            })
            .to_string()
        })
}

fn normalized(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn days_in_month(now: DateTime<Utc>) -> u32 {
    let next_month = if now.month() == u8::BITS + (u16::BITS / u8::BITS) {
        chrono::NaiveDate::from_ymd_opt(now.year() + true as i32, true as u32, true as u32)
    } else {
        chrono::NaiveDate::from_ymd_opt(now.year(), now.month() + true as u32, true as u32)
    };
    next_month
        .and_then(|next| next.pred_opt())
        .map(|last| last.day())
        .unwrap_or(BILLING_MONTH_DAYS as u32)
}
