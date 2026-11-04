//! What the Vast marketplace asks for a board model right now, read from its
//! public offer search (`GET /bundles/`, the endpoint `vastai search offers`
//! calls), which needs no key: a fleet with no Vast account can still see
//! what its boards would rent for before it lists anything.

use serde::Serialize;
use serde_json::{json, Value};

use crate::providers::vast::{VastError, VAST_BASE};

/// The rentable one-board on-demand offers of `gpu_name`, summarised.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MarketPrice {
    pub gpu_name: String,
    pub offers: usize,
    /// US dollars per hour a renter pays (`dph_total`), lowest, median and
    /// highest.
    pub min_usd_per_hour: Option<f64>,
    pub median_usd_per_hour: Option<f64>,
    pub max_usd_per_hour: Option<f64>,
    pub read_at: String,
    pub source: String,
}

/// Read the rentable one-board on-demand offers of `gpu_name` (Vast's own
/// spelling, as `nvidia-smi` and the offers name it).
pub async fn read(gpu_name: &str) -> Result<MarketPrice, VastError> {
    let query = json!({
        "gpu_name": {"eq": gpu_name},
        "rentable": {"eq": true},
        "num_gpus": {"eq": 1},
        "type": "on-demand",
    });
    let url = format!("{VAST_BASE}/bundles/");
    let request = reqwest::Client::new()
        .get(&url)
        .query(&[("q", query.to_string())])
        .header(reqwest::header::ACCEPT, "application/json");
    let response = crate::wait::request(request).await?;
    let status = response.status();
    let text = response.text().await.map_err(|error| VastError::Api {
        status: None,
        detail: format!("Vast.ai GET /bundles/ -> HTTP {status}, body unreadable: {error}"),
    })?;
    if !status.is_success() {
        return Err(VastError::Api {
            status: Some(status.as_u16()),
            detail: format!("Vast.ai GET /bundles/ -> HTTP {status}: {text}"),
        });
    }
    let body: Value = serde_json::from_str(&text).map_err(|error| VastError::Api {
        status: None,
        detail: format!("Vast.ai GET /bundles/ -> invalid JSON: {error}"),
    })?;
    let offers = body
        .get("offers")
        .and_then(Value::as_array)
        .ok_or_else(|| VastError::Api {
            status: None,
            detail: format!("Vast.ai GET /bundles/ answered no offers list: {body}"),
        })?;
    let mut prices: Vec<f64> = offers
        .iter()
        .filter_map(|offer| offer.get("dph_total").and_then(Value::as_f64))
        .filter(|price| price.is_finite() && *price > 0.0)
        .collect();
    prices.sort_by(f64::total_cmp);
    Ok(MarketPrice {
        gpu_name: gpu_name.to_string(),
        offers: offers.len(),
        min_usd_per_hour: prices.first().copied(),
        median_usd_per_hour: prices.get(prices.len() / 2).copied(),
        max_usd_per_hour: prices.last().copied(),
        read_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        source: format!("{url} (rentable, one board, on-demand)"),
    })
}
