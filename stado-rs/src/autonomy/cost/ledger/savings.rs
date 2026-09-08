//! The savings summary: predicted against realized, per provider.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::autonomy::model::{SavingsMeasurement, SavingsRecord};

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
