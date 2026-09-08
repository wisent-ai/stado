use super::*;

mod head;
mod tail;

use head::LAUNCH_WORKER_SCRIPT_HEAD;
use tail::LAUNCH_WORKER_SCRIPT_TAIL;

pub(super) fn launch_worker_script(
    transaction: &str,
    staged_tool: &str,
    canonical_tool: &str,
    tool_sha256: &str,
    arguments: &[String],
) -> Result<String, DeployError> {
    let arguments = serde_json::to_vec(arguments)
        .map_err(|error| DeployError(format!("cannot encode worker arguments: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(arguments);
    Ok(
        format!("{LAUNCH_WORKER_SCRIPT_HEAD}\n{LAUNCH_WORKER_SCRIPT_TAIL}")
            .replace("@ARGS@", &shlex_quote(&encoded))
            .replace("@STAGED@", &shlex_quote(staged_tool))
            .replace("@TOOL@", &shlex_quote(canonical_tool))
            .replace("@SHA@", &shlex_quote(tool_sha256))
            .replace("@FENCE_SCHEMA@", FENCE_SCHEMA)
            .replace("@TX@", &shlex_quote(transaction)),
    )
}
