//! The [`PriceBook`] lookup: the cheapest quote that fits a resource.
//!
//! [`PriceBook::find_hourly`] answers with an exact machine or accelerator
//! match where the catalog has one, and on GCP falls back to composing a
//! machine from its core, memory and accelerator SKUs — which is what the
//! shape table at the bottom describes.

use crate::autonomy::model::SCHEMA_VERSION;
use crate::capabilities::ProviderId;

use super::{normalized, PriceBook, PriceQuote};

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
