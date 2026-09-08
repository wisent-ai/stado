//! The cost documents this module publishes, and the billing snapshot the
//! forecast reads back.

use serde_json::Value;

use crate::autonomy::cost::allocation::AllocationReport;
use crate::autonomy::cost::forecast::{CostAnomaly, CostForecast};
use crate::autonomy::cost::prices::PriceBook;
use crate::queue::{JobStorage, StorageError};

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
