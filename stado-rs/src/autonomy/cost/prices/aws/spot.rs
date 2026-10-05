use super::super::PriceQuote;
use aws_sdk_ec2::{primitives::DateTime as ProviderTime, types::InstanceType, Client};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

/// EC2 returns the last change before StartTime as the effective price at that
/// instant. Read every page and retain each zone, not a median across locations.
/// https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_DescribeSpotPriceHistory.html
pub(super) async fn read(
    client: &Client,
    instance_types: Vec<InstanceType>,
    observed_at: DateTime<Utc>,
) -> Result<Vec<PriceQuote>, String> {
    let at = ProviderTime::from_secs_and_nanos(
        observed_at.timestamp(),
        observed_at.timestamp_subsec_nanos(),
    );
    let request = client
        .describe_spot_price_history()
        .set_instance_types(Some(instance_types))
        .product_descriptions("Linux/UNIX")
        .start_time(at)
        .end_time(at);
    let mut token = None;
    let mut latest = BTreeMap::<(String, String), (ProviderTime, f64)>::new();
    loop {
        let output = request
            .clone()
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| format!("EC2 DescribeSpotPriceHistory: {error}"))?;
        for item in output.spot_price_history() {
            let machine = item
                .instance_type()
                .map(|kind| kind.as_str())
                .ok_or_else(|| format!("EC2 spot price omitted instance type: {item:?}"))?;
            let zone = item
                .availability_zone()
                .filter(|zone| !zone.is_empty())
                .ok_or_else(|| format!("EC2 spot price omitted availability zone: {item:?}"))?;
            let timestamp = item
                .timestamp()
                .copied()
                .ok_or_else(|| format!("EC2 spot price omitted effective timestamp: {item:?}"))?;
            let price = item
                .spot_price()
                .and_then(|price| price.parse::<f64>().ok())
                .filter(|price| price.is_finite() && *price >= 0.0)
                .ok_or_else(|| {
                    format!("EC2 spot price is not a nonnegative finite amount: {item:?}")
                })?;
            if timestamp > at {
                continue;
            }
            let current = latest
                .entry((machine.into(), zone.into()))
                .or_insert((timestamp, price));
            if timestamp > current.0 {
                *current = (timestamp, price);
            } else if timestamp == current.0 && price != current.1 {
                return Err(format!("EC2 returned conflicting effective spot prices for {machine} in {zone} at {timestamp}: {} and {price}", current.1));
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
    Ok(latest
        .into_iter()
        .map(|((machine, zone), (effective_at, rate))| {
            let mut quote = super::aws_quote(
                &machine,
                "spot",
                rate,
                &zone,
                super::SPOT_SOURCE,
                observed_at,
            );
            quote.description =
                format!("AWS EC2 {machine} spot in {zone}; effective since {effective_at}");
            quote
        })
        .collect())
}
