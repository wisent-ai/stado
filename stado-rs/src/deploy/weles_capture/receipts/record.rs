//! Stado's record of one capture batch: every run it started and what that run
//! answered, stored at `stado://weles-captures/<batch>/batch-record.json`.
//!
//! Weles keeps no queryable history of the runs it executed; Stado started
//! every capture run and holds its answer, so Stado writes it down. The record
//! is rewritten after every run, so a batch stopped halfway still reports the
//! runs it finished and the refusal that stopped it.
//!
//! Only one invocation ever writes a batch's record: it first creates
//! `batch-claim.json` holding a token of its own, create-only, and a batch
//! whose claim exists is refused. A retry or a concurrent run of the same
//! plan therefore cannot replace the receipts an earlier run wrote.

use serde::{Deserialize, Serialize};

use super::super::ARTIFACT_NAMESPACE;
use crate::deploy::DeployError;

const RECORD_OBJECT: &str = "batch-record.json";
const CLAIM_OBJECT: &str = "batch-claim.json";

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

fn object_uri(batch: &str, object: &str) -> String {
    format!("stado://{ARTIFACT_NAMESPACE}/{batch}/{object}")
}

fn record_uri(batch: &str) -> String {
    object_uri(batch, RECORD_OBJECT)
}

async fn put(
    uri: &str,
    bytes: &[u8],
    content_type: &str,
    if_absent: bool,
) -> Result<(), DeployError> {
    let staged = tempfile::NamedTempFile::new()
        .map_err(DeployError::io(format!("cannot stage {uri}")))?;
    std::fs::write(staged.path(), bytes).map_err(DeployError::io(format!(
        "cannot stage {uri} at {}",
        staged.path().display()
    )))?;
    crate::cli::storage::store_object(
        uri,
        &staged.path().display().to_string(),
        content_type,
        if_absent,
    )
    .await
    .map(|_| ())
    .map_err(DeployError::from)
}

/// Make this invocation the only writer of the batch's record. The claim is
/// created only if absent and holds a token no other invocation has, so a
/// second claim of the same batch - a retry or a concurrent run - is refused
/// instead of replacing receipts it never wrote.
pub async fn claim(batch: &str) -> Result<(), DeployError> {
    let uri = object_uri(batch, CLAIM_OBJECT);
    let started = || {
        DeployError(format!(
            "capture batch {batch} was already started ({uri} exists); a batch runs once, \
             so plan a new batch id, and read this one with \
             `stado workload status weles-capture:{batch}`"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused)
    };
    match crate::cli::storage::fetch_object_versioned(&uri).await {
        Ok(Some(_)) => return Err(started()),
        Ok(None) => {}
        Err(error) => return Err(DeployError::from(error).within(format!("cannot read {uri}"))),
    }
    let token = uuid::Uuid::new_v4().to_string();
    put(&uri, token.as_bytes(), "text/plain", true)
        .await
        .map_err(|error| error.within(format!("cannot claim {uri}")))
}

/// Replace the batch's record with every receipt so far. Only the invocation
/// that won [`claim`] calls this.
pub async fn write(batch: &str, receipts: &[Receipt]) -> Result<(), DeployError> {
    let uri = record_uri(batch);
    let bytes = serde_json::to_vec(receipts)
        .map_err(|error| DeployError(format!("cannot encode {uri}: {error}")))?;
    put(&uri, &bytes, "application/json", false)
        .await
        .map_err(|error| error.within(format!("cannot store {uri}")))
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
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound))
        }
        Err(error) => return Err(DeployError::from(error).within(format!("cannot read {uri}"))),
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!("{uri} is not a capture batch record: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
