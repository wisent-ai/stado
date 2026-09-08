//! The price records and the provider reads that fill them.
//!
//! The records — [`PriceState`], [`PriceQuote`], [`PriceSource`] and
//! [`PriceBook`] — and [`refresh_prices`], which fans out to one reader per
//! configured provider, are here. [`book`] holds the [`PriceBook`] lookup and
//! the machine shape table it composes from; [`gcp`], [`azure`] and [`aws`]
//! hold the reads themselves. The description helpers at the bottom are the
//! vocabulary those readers share.

mod aws;
mod azure;
mod book;
mod gcp;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::autonomy::model::SCHEMA_VERSION;
use crate::autonomy::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;

use aws::aws_spot_prices;
use azure::azure_prices;
use gcp::gcp_prices;

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
