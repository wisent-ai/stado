//! What `stado resolver status` says about the registry authority beyond
//! the probe: why it is a blocker, and what this machine remembers about
//! reading it, so an operator sees why a command answered from a kept
//! snapshot or refused a write without asking the authority.

use serde_json::{json, Value};

use crate::cli::resolver::directory::remembered::{recalled, unanswered};
use crate::cli::resolver::report::probe::AuthorityAnswer;
use crate::service_resolution::ServiceAuthority;

/// The blockers the authority's answer raises: it did not answer, or it
/// answered only after longer than the refresh interval this target
/// declares, which cannot have refreshed the snapshot in time.
pub(super) fn blockers(
    authority: &AuthorityAnswer,
    declared: &ServiceAuthority,
    refresh_seconds: u64,
) -> Vec<String> {
    let mut blockers = Vec::new();
    if !authority.reachable {
        let mut sentence = format!("the registry authority {} is unreachable", declared.target);
        if let Some(detail) = authority.detail.as_deref() {
            sentence.push_str(": ");
            sentence.push_str(detail);
        }
        blockers.push(sentence);
    }
    if let Some(seconds) = authority
        .answered_after_seconds
        .filter(|seconds| *seconds > refresh_seconds)
    {
        blockers.push(format!(
            "the registry authority {} answered `{} resolver snapshot` over SSH only after {seconds}s, \
             longer than the {refresh_seconds}s refresh interval this target declares",
            declared.target, declared.command
        ));
    }
    blockers
}

/// The blocker for a host that is the registry authority by its own copy
/// while its registry store is an object API another host serves
/// ([`crate::cli::resolver::authority_store_elsewhere`]); a copy that cannot
/// be read is itself the blocker. `None` on a host whose store is not an
/// object API.
pub(super) fn store_blocker() -> Option<String> {
    let adapter = crate::capabilities::storage_adapter(crate::config::wc_storage_backend());
    if adapter != Some(crate::capabilities::StorageAdapter::StadoObject) {
        return None;
    }
    let store = match crate::config::wc_stado_storage_url() {
        Ok(store) => store,
        Err(cause) => return Some(format!("this host's registry store cannot be named: {cause}")),
    };
    match crate::cli::resolver::authority_store_elsewhere(&store) {
        Ok(sentence) => sentence,
        Err(cause) => Some(format!(
            "whether this host's registry store {store} is held by the registry authority \
             cannot be told: {cause}"
        )),
    }
}

/// The blocker for a host whose own object store (`~/.stado/local-storage`,
/// which its Stado serves the object API from) is fenced: every write to it
/// is refused, as a role move leaves the host it moved away from, or as a
/// storage-root reconcile holds it. `None` when it is not fenced; a fence
/// that cannot be read is itself the blocker.
pub(super) fn fenced_store_blocker() -> Option<String> {
    let home = match crate::config_file::home() {
        Ok(home) => home,
        Err(cause) => return Some(format!("whether this host's object store is fenced cannot be told: {cause}")),
    };
    let root = home.join(crate::config_file::LOCAL_STORAGE_ROOT);
    let state = match crate::queue::LocalBackend::write_fence_state(&root) {
        Ok(state) => state,
        Err(error) => {
            return Some(format!(
                "whether this host's object store {} is fenced cannot be told: {error}",
                root.display()
            ))
        }
    };
    let intent = state.get("intent").filter(|intent| !intent.is_null())?;
    Some(format!(
        "this host's object store {} refuses every write: it is fenced by {} transaction {} \
         ({}); stado storage fence status names it",
        root.display(),
        crate::cli::reporting::field(&intent["owner"]),
        crate::cli::reporting::field(&intent["transaction"]),
        crate::cli::reporting::field(&intent["reason"])
    ))
}

/// What this machine keeps about reads of the authority: the given-up read
/// that stops every command asking it until the interval ends, and the
/// snapshot every registry read answers from until then.
pub(super) fn remembered(declared: &ServiceAuthority) -> Value {
    let kept = recalled::kept(&declared.target).map(|(version, until)| {
        json!({ "version": version, "until": until.to_rfc3339() })
    });
    json!({
        "unanswered": unanswered::standing(&declared.target),
        "kept_snapshot": kept,
    })
}
