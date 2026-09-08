//! Savings records and the measurements that check them, each written once
//! and each answering "is this one already measured?" from its own name.

use std::collections::BTreeSet;

use crate::autonomy::model::{SavingsMeasurement, SavingsRecord};
use crate::autonomy::storage::objects::{list_record_ids, load_records, write_json};
use crate::autonomy::storage::{SAVINGS_MEASUREMENT_PREFIX, SAVINGS_PREFIX};
use crate::queue::{JobStorage, StorageError};

pub async fn write_savings(
    store: &JobStorage,
    savings: &SavingsRecord,
) -> Result<(), StorageError> {
    write_json(
        store,
        &format!("{SAVINGS_PREFIX}/{}.json", savings.savings_id),
        savings,
        true,
    )
    .await
}

pub async fn list_savings(store: &JobStorage) -> Result<Vec<SavingsRecord>, StorageError> {
    load_records(store, &format!("{SAVINGS_PREFIX}/")).await
}
pub async fn write_savings_measurement(
    store: &JobStorage,
    measurement: &SavingsMeasurement,
) -> Result<(), StorageError> {
    write_json(
        store,
        &format!(
            "{SAVINGS_MEASUREMENT_PREFIX}/{}.json",
            measurement.measurement_id
        ),
        measurement,
        true,
    )
    .await
}

pub async fn list_savings_measurements(
    store: &JobStorage,
) -> Result<Vec<SavingsMeasurement>, StorageError> {
    load_records(store, &format!("{SAVINGS_MEASUREMENT_PREFIX}/")).await
}

/// Every savings id currently recorded.
pub async fn list_savings_ids(store: &JobStorage) -> Result<BTreeSet<String>, StorageError> {
    list_record_ids(store, &format!("{SAVINGS_PREFIX}/")).await
}

/// Savings ids that already carry a measurement. A measurement is written as
/// `measurement-<savings id>`, so its name names the saving it measured.
pub async fn list_measured_savings_ids(
    store: &JobStorage,
) -> Result<BTreeSet<String>, StorageError> {
    Ok(
        list_record_ids(store, &format!("{SAVINGS_MEASUREMENT_PREFIX}/"))
            .await?
            .into_iter()
            .filter_map(|id| id.strip_prefix("measurement-").map(str::to_string))
            .collect(),
    )
}

/// One savings record by id.
pub async fn load_savings(
    store: &JobStorage,
    savings_id: &str,
) -> Result<Option<SavingsRecord>, StorageError> {
    let Some(raw) = store
        .download_text(&format!("{SAVINGS_PREFIX}/{savings_id}.json"))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}
