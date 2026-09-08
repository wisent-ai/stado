//! The queue plan one platform's build job is: its immutable request, the
//! bootstrap command that carries it, and the terminal job it becomes.

mod command;
mod enqueue;
pub(in crate::cli::release_submit) mod platforms;
pub(in crate::cli::release_submit) mod terminal;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::models::JobSecretRef;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::WorkerRequest;

/// Keep the first complete request for an attempt, including its builder.
/// Capacity can change between publication and submission, or two coordinators
/// can race. Only placement may differ; the source and recipe must still agree.
async fn persist_worker_request(
    store: &JobStorage,
    path: &str,
    mut expected: WorkerRequest,
    saved: Option<(WorkerRequest, Vec<u8>)>,
) -> Result<(WorkerRequest, Vec<u8>), CmdError> {
    let (request, bytes) = match saved {
        Some(saved) => saved,
        None => {
            let content = serde_json::to_string(&expected)?;
            if store
                .create_text_if_absent(path, &content)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?
            {
                return Ok((expected, content.into_bytes()));
            }
            let bytes = store
                .read_bytes(path)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "worker request disappeared after concurrent publication: {path}"
                    ))
                })?;
            (serde_json::from_slice::<WorkerRequest>(&bytes)?, bytes)
        }
    };
    expected.builder.clone_from(&request.builder);
    if request != expected {
        return Err(CmdError::click(format!(
            "immutable queue object differs: {path}"
        )));
    }
    Ok((request, bytes))
}
pub(crate) fn secret_refs(v: &BTreeMap<String, String>) -> BTreeMap<String, JobSecretRef> {
    v.iter()
        .filter_map(|(n, r)| {
            r.split_once('#').map(|(i, f)| {
                (
                    n.clone(),
                    JobSecretRef {
                        item: i.into(),
                        field: f.into(),
                    },
                )
            })
        })
        .collect()
}
pub(crate) fn input(uri: &str, path: &str, sha: &str) -> Value {
    json!({"stado_uri":uri,"relative_path":path,"sha256":sha})
}
