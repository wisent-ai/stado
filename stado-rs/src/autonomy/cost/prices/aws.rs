//! AWS provider observations: effective zonal Spot prices and every matching
//! on-demand price dimension. Neither history medians nor cheapest-price guesses.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::capabilities::ProviderId;

use super::{PriceQuote, PriceSource, PriceState};
mod spot;

pub(in crate::autonomy::cost) const SPOT_SOURCE: &str =
    "EC2 DescribeSpotPriceHistory effective zonal price";
pub(in crate::autonomy::cost) const ON_DEMAND_SOURCE: &str =
    "AWS Price List GetProducts flat price dimension";

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
    match spot::read(&client, instance_types, observed_at).await {
        Ok(quotes) => source.quotes.extend(quotes),
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
    let mut quotes = Vec::new();
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
            let value: Value = serde_json::from_str(raw).map_err(|error| {
                format!("AWS Price List returned invalid JSON: {error}; body={raw}")
            })?;
            let machine = value
                .pointer("/product/attributes/instanceType")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("AWS Price List omitted instanceType: {value}"))?;
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
                    if dimension.get("beginRange").and_then(Value::as_str) != Some("0")
                        || dimension.get("endRange").and_then(Value::as_str) != Some("Inf")
                    {
                        return Err(format!("AWS Price List hourly dimension is tiered or omitted its range; no flat rate was chosen: {dimension}"));
                    }
                    let rate = dimension.pointer("/pricePerUnit/USD").and_then(Value::as_str)
                        .and_then(|raw| raw.parse::<f64>().ok()).filter(|rate| rate.is_finite() && *rate >= 0.0)
                        .ok_or_else(|| format!("AWS Price List hourly dimension has no finite nonnegative USD amount: {dimension}"))?;
                    let code = dimension
                        .get("rateCode")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            format!("AWS Price List omitted the provider rate code: {dimension}")
                        })?;
                    let mut quote = aws_quote(
                        machine,
                        "on_demand",
                        rate,
                        region,
                        ON_DEMAND_SOURCE,
                        observed_at,
                    );
                    quote.sku = code.to_owned();
                    quotes.push(quote);
                }
            }
        }
        token = output
            .next_token()
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    Ok(quotes)
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
        provider: ProviderId::Aws,
        sku: machine.to_string(),
        description: format!("AWS EC2 {machine} {purchase_option}"),
        region: Some(region.to_string()),
        machine_type: Some(machine.to_string()),
        accelerator_type: crate::catalog::AWS_INSTANCE_TO_ACCEL
            .get(machine)
            .map(|accelerator| accelerator.to_string()),
        family: None,
        purchase_option: purchase_option.to_string(),
        unit: "hour".to_string(),
        hourly_usd,
        currency: "USD".to_string(),
        source: source.to_string(),
        observed_at: observed_at.to_rfc3339(),
        dynamic: true,
    }
}
