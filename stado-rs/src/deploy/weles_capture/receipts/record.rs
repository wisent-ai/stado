//! Stado's record of one capture batch: every run it started and what that run
//! answered, stored at `stado://weles-captures/<batch>/batch-record.json`.
//!
//! Weles keeps no queryable history of the runs it executed; Stado started
//! every capture run and holds its answer, so Stado writes it down. The record
//! is rewritten after every run, so a batch stopped halfway still reports the
//! runs it finished and the refusal that stopped it.

use serde::{Deserialize, Serialize};

use super::super::ARTIFACT_NAMESPACE;
use crate::deploy::DeployError;

const RECORD_OBJECT: &str = "batch-record.json";

/// One capture run as Stado recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub run_id: String,
    pub site_slug: String,
    pub axis: String,
    pub artifact_prefix: String,
    pub state: String,
    pub error: Option<String>,
}

fn record_uri(batch: &str) -> String {
    format!("stado://{ARTIFACT_NAMESPACE}/{batch}/{RECORD_OBJECT}")
}

/// Replace the batch's record with every receipt so far.
pub async fn write(batch: &str, receipts: &[Receipt]) -> Result<(), DeployError> {
    let uri = record_uri(batch);
    let bytes = serde_json::to_vec(receipts)
        .map_err(|error| DeployError(format!("cannot encode {uri}: {error}")))?;
    let staged = tempfile::NamedTempFile::new()
        .map_err(|error| DeployError(format!("cannot stage {uri}: {error}")))?;
    std::fs::write(staged.path(), &bytes)
        .map_err(|error| DeployError(format!("cannot stage {uri}: {error}")))?;
    crate::cli::storage::store_object(
        &uri,
        &staged.path().display().to_string(),
        "application/json",
        false,
    )
    .await
    .map_err(|error| DeployError(format!("cannot store {uri}: {error}")))?;
    Ok(())
}

/// The batch's record. A batch Stado never recorded is refused as unknown,
/// never reported as empty; a record the store cannot read now (auth, network,
/// a 503) is reported as unreadable, never as unknown.
pub async fn read(batch: &str) -> Result<Vec<Receipt>, DeployError> {
    let uri = record_uri(batch);
    let bytes = match crate::cli::storage::fetch_object_versioned(&uri).await {
        Ok(Some((bytes, _version))) => bytes,
        Ok(None) => {
            return Err(DeployError(format!(
                "capture batch {batch} is unknown: Stado holds no record at {uri}"
            )))
        }
        Err(error) => return Err(DeployError(format!("cannot read {uri}: {error}"))),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| DeployError(format!("{uri} is not a capture batch record: {error}")))
}
