//! The [`PriceBook`] lookup: the cheapest quote that fits a resource.
//!
//! [`PriceBook::find_hourly`] answers with an exact machine or accelerator
//! match where the catalog has one, and on GCP falls back to composing a
//! machine from its core, memory and accelerator SKUs — which is what the
//! shape table at the bottom describes.

use crate::capabilities::ProviderId;

use super::{PriceBook, PriceQuote};

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
                && quote.hourly_usd > 0.0
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
            .filter(|quote| quote.machine_type.as_deref() == Some(machine_type))
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
            .filter(|quote| quote.accelerator_type.as_deref() == Some(accelerator_type))
            .min_by(|left, right| {
                left.hourly_usd
                    .partial_cmp(&right.hourly_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// The cheapest quoted hourly price of one accelerator on any provider,
    /// for the purchase `preemptible` names; `None` when no quote names it.
    /// What keeping a job on owned hardware saves is what the cheapest cloud
    /// would have charged for its accelerator.
    pub fn cheapest_accelerator_hourly(&self, accelerator_type: &str, preemptible: bool) -> Option<f64> {
        let purchase = if preemptible { "spot" } else { "on_demand" };
        self.quotes
            .iter()
            .filter(|quote| {
                quote.purchase_option == purchase
                    && quote.accelerator_type.as_deref() == Some(accelerator_type)
                    && quote.hourly_usd.is_normal()
                    && quote.hourly_usd.is_sign_positive()
            })
            .map(|quote| quote.hourly_usd)
            .min_by(f64::total_cmp)
    }

    fn gcp_composite_hourly(
        &self,
        region: Option<&str>,
        machine_type: &str,
        accelerator_type: &str,
        purchase: &str,
    ) -> Option<PriceQuote> {
        let (family, cores, memory_gb, accelerator_count) = gcp_machine_shape(machine_type)?;
        // The family and the accelerator come from the SKU's product
        // taxonomy; whether a SKU prices a core hour or a GiB-hour of memory
        // is its usage unit.
        let core = self.cheapest_gcp_quote(region, purchase, |quote| {
            quote.family.as_deref() == Some(family) && quote.unit == "hour"
        })?;
        let memory = self.cheapest_gcp_quote(region, purchase, |quote| {
            quote.family.as_deref() == Some(family) && quote.unit == "gib_hour"
        })?;
        let accelerator = self.cheapest_gcp_quote(region, purchase, |quote| {
            quote.accelerator_type.as_deref() == Some(accelerator_type)
        })?;
        Some(PriceQuote {
            provider: ProviderId::Gcp,
            sku: format!("{}+{}+{}", core.sku, memory.sku, accelerator.sku),
            description: format!(
                "GCP {machine_type} + {accelerator_type} composed from live Billing SKUs"
            ),
            region: region.map(str::to_string),
            machine_type: Some(machine_type.to_string()),
            accelerator_type: Some(accelerator_type.to_string()),
            family: None,
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
                    && quote.hourly_usd > 0.0
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
