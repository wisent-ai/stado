//! Every read `release status` waits on, said while it waits.
//!
//! The command read the registry, every target's release state and every
//! recorded release run, and printed nothing until all of them answered: a
//! status that ran for half an hour could not be told from one that was
//! stuck, nor the read it was stuck on. Each read now names itself (which
//! object, which product and host, how many runs) on stderr when it starts and
//! says how long it took when it ends, error or not; stdout, and with it
//! `--json`, is unchanged.

use serde_json::Value;

use crate::cli::build_cmd::timing::phase_of;
use crate::cli::release_submit::RunFilter;
use crate::cli::CmdError;

const COMMAND: &str = "release status";

/// How a phase names the runs a window reads.
fn window(limit: usize) -> String {
    match limit {
        usize::MAX => "every recorded release run".to_string(),
        limit => format!("the newest {limit} release runs"),
    }
}

pub(super) async fn registry_document() -> Result<Value, CmdError> {
    let _phase = phase_of(COMMAND, "read the registry document");
    crate::cli::registry::fetch_document().await
}

pub(super) async fn registry_targets() -> Result<crate::targets::Registry, CmdError> {
    let _phase = phase_of(COMMAND, "read the registry's targets");
    crate::targets::fetch_registry_remote()
        .await
        .map_err(CmdError::from)
}

/// The release state `target` last wrote for `product`; `None` when it wrote
/// none that this host can read.
pub(super) async fn release_state(product: &str, target: &str) -> Option<Vec<u8>> {
    let uri = crate::release_agent::release_status_uri(product, target);
    let _phase = phase_of(COMMAND, format!("read {product} on {target} ({uri})"));
    crate::cli::storage::fetch_object(&uri).await.ok()
}

pub(super) async fn recent_runs(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    let _phase = phase_of(COMMAND, format!("read {}", window(limit)));
    crate::cli::release_submit::recent_runs(product, limit).await
}

pub(super) async fn matching_runs(
    filter: RunFilter<'_>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    let _phase = phase_of(
        COMMAND,
        format!("search {} for the run asked", window(limit)),
    );
    crate::cli::release_submit::matching_runs(filter, limit).await
}
