//! `stado machine ...` — stable JSON machine interface.
//!
//! Port of the `machine` click group in `stado/machine.py`. Every command
//! prints EXACTLY ONE JSON envelope line on stdout —
//! `{"schema_version":1,"ok":true,"result":...}` on success,
//! `{"schema_version":1,"ok":false,"error":{code,message,retryable}}` on
//! failure (exit 1, click `Exit(1)`) — serialized with
//! [`canonical_json`] (Python `_canonical_json`: sort_keys, compact
//! separators, ensure_ascii=False). stderr stays clean for automation.

use serde_json::Value;

use crate::machine::{canonical_json, MachineError, MachineFacade, SCHEMA_VERSION};

use super::CmdError;

/// Python `_emit`: one canonical-JSON line on stdout.
fn emit(payload: Value) {
    println!("{}", canonical_json(&payload));
}

/// Python `_invoke`: run the operation, emit the success or error envelope,
/// exit 1 on any failure.
async fn invoke(
    operation: impl std::future::Future<Output = Result<Value, MachineError>>,
) -> Result<(), CmdError> {
    match operation.await {
        Ok(result) => {
            emit(serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "result": result,
            }));
            Ok(())
        }
        Err(exc) => {
            emit(serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "error": {"code": exc.code, "message": exc.message, "retryable": exc.retryable},
            }));
            Err(CmdError::silent(1))
        }
    }
}

/// `machine submit --request-file PATH`: submit one idempotent request from
/// a JSON file.
pub async fn submit(request_file: &str) -> Result<(), CmdError> {
    invoke(async {
        let unreadable = |exc: std::io::Error| {
            MachineError::new("INVALID_REQUEST", format!("cannot read request JSON: {exc}"))
        };
        let raw = std::fs::read_to_string(request_file).map_err(unreadable)?;
        let request: Value = serde_json::from_str(&raw).map_err(|exc| {
            MachineError::new("INVALID_REQUEST", format!("cannot read request JSON: {exc}"))
        })?;
        MachineFacade::new().await?.submit_request(&request).await
    })
    .await
}

/// `machine status JOB_ID`: read one job directly by ID.
pub async fn status(job_id: &str) -> Result<(), CmdError> {
    let job_id = job_id.to_string();
    invoke(async move { MachineFacade::new().await?.status(&job_id).await }).await
}

/// `machine logs JOB_ID --cursor N --limit N`: read a byte-cursor page from
/// the canonical command log.
pub async fn logs(job_id: &str, cursor: i64, limit: i64) -> Result<(), CmdError> {
    let job_id = job_id.to_string();
    invoke(async move { MachineFacade::new().await?.read_logs(&job_id, cursor, limit).await }).await
}

/// `machine cancel JOB_ID`: durably and idempotently cancel one job.
pub async fn cancel(job_id: &str) -> Result<(), CmdError> {
    let job_id = job_id.to_string();
    invoke(async move { MachineFacade::new().await?.cancel_job(&job_id).await }).await
}

/// `machine artifacts JOB_ID --output-dir DIR`: download and verify
/// canonical artifacts for a terminal job.
pub async fn artifacts(job_id: &str, output_dir: &str) -> Result<(), CmdError> {
    let job_id = job_id.to_string();
    let output_dir = std::path::PathBuf::from(output_dir);
    invoke(async move { MachineFacade::new().await?.download_artifacts(&job_id, &output_dir).await })
        .await
}
