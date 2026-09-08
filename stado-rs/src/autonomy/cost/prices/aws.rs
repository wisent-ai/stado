//! The AWS price read: the median of the EC2 spot history plus the Price
//! List on-demand rate, for the instance types the catalog knows.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::autonomy::model::SCHEMA_VERSION;
use crate::capabilities::ProviderId;

use super::{PriceQuote, PriceSource, PriceState};

pub(super) async fn aws_spot_prices(observed_at: DateTime<Utc>) -> PriceSource {
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
