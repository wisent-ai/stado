//! The completed-refs scan Branch B tests a never-worked VM against, and the
//! reason it is rebuilt per tick here instead of cached in a global.

use std::collections::HashSet;

use crate::queue::JobStorage;

use super::super::MonitorError;

/// Return set of instance_ref strings appearing in completed/.
///
/// DEVIATION from Python: `_instance_refs_with_completions` keeps the set
/// in a process-global 300s-TTL cache because the Cloud Function is
/// short-lived. Here the set is rebuilt on every sweep that has a running
/// VM to judge.
pub(super) async fn instance_refs_with_completions(
    store: &JobStorage,
    kind: &str,
) -> Result<HashSet<String>, MonitorError> {
    // Python keeps `kind` in the signature (unused in the body there too).
    let _ = kind;
    let mut refs = HashSet::new();
    for job in store.list_jobs("completed", 0).await? {
        if let Some(r) = job.instance_ref.filter(|r| !r.is_empty()) {
            refs.insert(r);
        }
    }
    Ok(refs)
}
