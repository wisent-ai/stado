//! The instructions an `omp-session` starts with (`--context FILE`): the
//! files read here, in order, made into the harness's own user-level context
//! file, published to the job store and declared as the job's input, so the
//! host's agent writes it into the session's agent directory before the
//! session starts. The instructions this machine's sessions read live on this
//! machine; a session on another host gets them this way, not from a copy
//! left in that host's home.

use std::path::PathBuf;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::command::AGENT_DIRECTORY;
use crate::cli::CmdError;

/// The harness's user-level context file inside its agent directory.
const CONTEXT_FILE: &str = "AGENTS.md";

/// The job input the context is declared as.
const INPUT_NAME: &str = "harness-context";

/// Publish `files` for the session `run_id` and return the job inputs that
/// carry them; nothing when no file was named. A file that cannot be read is
/// refused naming it, before anything is published.
pub(super) async fn inputs(
    run_id: &str,
    files: &[PathBuf],
) -> Result<Map<String, Value>, CmdError> {
    let mut inputs = Map::new();
    if files.is_empty() {
        return Ok(inputs);
    }
    let mut document = String::new();
    for file in files {
        let text = std::fs::read_to_string(file).map_err(|error| {
            CmdError::usage(format!(
                "--context {} cannot be read: {error}",
                file.display()
            ))
        })?;
        document.push_str(&text);
        if !text.ends_with('\n') {
            document.push('\n');
        }
        document.push('\n');
    }
    let key = format!("workload-sessions/{run_id}/{CONTEXT_FILE}");
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| {
            CmdError::click(format!(
            "the job store the session's instructions are published to cannot be opened: {error}"
        ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    store.upload_bytes(&key, document.as_bytes()).await?;
    inputs.insert(
        INPUT_NAME.to_string(),
        json!({
            "stado_uri": format!(
                "stado://{}/{key}",
                crate::config::wc_stado_storage_namespace()
            ),
            "relative_path": format!("{AGENT_DIRECTORY}/{CONTEXT_FILE}"),
            "sha256": hex::encode(Sha256::digest(document.as_bytes())),
        }),
    );
    Ok(inputs)
}
