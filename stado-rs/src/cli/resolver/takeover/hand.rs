//! Where a registry naming a new authority is written: the store the new
//! authority reads, and every other host's last-known-good copy.

use serde_json::Value;

use crate::cli::resolver::authority::execute::execute_with_input;
use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::targets::{self, RegistryStore};

/// The state `registry push --last-known-good --json` answers with.
const HANDED_OVER: &str = "handed-over";

/// Write the promoted registry into the store this host reads, which its
/// copy now names it the authority of — or, given `at`, into the object API
/// at that address itself: created when absent, else swapped against the
/// version it holds.
pub(super) async fn stored(at: Option<&str>, content: &str) -> Result<String, String> {
    let opened = match at {
        Some(url) => RegistryStore::at_object_api(url),
        None => RegistryStore::open().await,
    };
    let store = opened.map_err(|error| format!("the registry store cannot be opened: {error}"))?;
    match store
        .read_versioned()
        .await
        .map_err(|error| format!("the registry in the store cannot be read: {error}"))?
    {
        Some(current) => store
            .compare_and_swap(&current.version, content)
            .await
            .map_err(|error| {
                format!(
                    "the registry in the store cannot be replaced from version {}: {error}",
                    current.version
                )
            }),
        None => match store.create_if_absent(content).await {
            Ok(true) => Ok("created".to_string()),
            Ok(false) => {
                Err("a registry appeared in the store while it was being created".to_string())
            }
            Err(error) => Err(format!("the registry cannot be created in the store: {error}")),
        },
    }
}

/// Hand `content` to one host's last-known-good copy over its SSH paths: the
/// host's receipt, or why no path took it. The paths are tried one after
/// another: a copy refuses a generation it already holds, so a second path
/// would only be refused.
pub(super) async fn handed(target: &targets::ComputeTarget, content: &str) -> Result<Value, String> {
    let remote = format!(
        "{} registry push - --last-known-good --json",
        crate::deploy::host_exec::STADO_CLI
    );
    let mut failures = Vec::new();
    for path in target_ssh_paths(target) {
        let output = match execute_with_input(&path.destination, &remote, content.as_bytes()).await
        {
            Ok(output) => output,
            Err(error) => {
                failures.push(format!("{}: {error:#}", path.name));
                continue;
            }
        };
        let exit = match output.exit_status {
            Some(status) => status.to_string(),
            None => "no exit status".to_string(),
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(receipt) if receipt["state"] == HANDED_OVER => return Ok(receipt),
            Ok(receipt) => failures.push(format!(
                "{}: exit {exit}, answered {receipt}: {}",
                path.name,
                stderr.trim()
            )),
            Err(error) => failures.push(format!(
                "{}: exit {exit}, no receipt ({error}): {}",
                path.name,
                stderr.trim()
            )),
        }
    }
    if failures.is_empty() {
        return Err(format!("{} declares no SSH connection path", target.name));
    }
    Err(failures.join("; "))
}
