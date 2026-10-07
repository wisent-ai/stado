use super::*;

/// The host script that launches the resident worker: the staged tool the
/// operator just synced runs `host storage-root-reconcile-host launch`,
/// which verifies its own digest, moves itself into the transaction's tool
/// path and starts the worker under the native manager.
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
    let staged_in_home = staged_tool.strip_prefix("$HOME/").ok_or_else(|| {
        DeployError("staged transaction tool is not under $HOME".to_string())
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok(format!(
        r#"set -euo pipefail
staged="$HOME"/{staged_in_home}
"$staged" host storage-root-reconcile-host launch --transaction {transaction} \
  --staged {staged} --tool {tool} --sha256 {sha256} --arguments {encoded}
"#,
        staged_in_home = shlex_quote(staged_in_home),
        transaction = shlex_quote(transaction),
        staged = shlex_quote(staged_tool),
        tool = shlex_quote(canonical_tool),
        sha256 = shlex_quote(tool_sha256),
        encoded = shlex_quote(&encoded),
    ))
}
