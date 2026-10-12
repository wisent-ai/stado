//! What other programs on this host read from the resolver's published
//! state: where an adapter listens, why a route they dialled is not served,
//! and the readiness marker of the host's roles.

use super::published::{published_state, PublishedState};

/// Why the resolver that published `state` left the adapter at `bind`
/// unserved, when it did.
pub(in crate::cli::resolver) fn unserved_reason(
    state: Option<&PublishedState>,
    bind: &str,
) -> Option<String> {
    state?
        .adapters
        .iter()
        .find(|held| held.bind == bind)?
        .unserved
        .clone()
}

/// `http://<bind>` of this host's adapter for `service` and `consumer`, as
/// the resolver holding its listeners bound published it; `None` while no
/// such resolver has published that adapter as served.
pub(crate) fn published_adapter_url(service: &str, consumer: &str) -> Option<String> {
    let state = published_state()?;
    if !state.listening {
        return None;
    }
    state
        .adapters
        .into_iter()
        .find(|adapter| {
            adapter.service == service && adapter.consumer == consumer && adapter.unserved.is_none()
        })
        .map(|adapter| format!("http://{}", adapter.bind))
}

/// Why this host's resolver does not serve `url`, in the words it
/// published: with no listener at all, the state, the process that wrote
/// it, when and its reason; with its listeners bound, the reason it left the
/// adapter at `url` unserved. `None` while no resolver has published, or the
/// one that did serves that address.
///
/// A connect refused on a resolver adapter's loopback bind reads as the
/// placed service failing to answer, while the socket nobody opened belongs
/// to a resolver that said why; naming that reason beside the refusal is what
/// makes the outage readable where it is met. A state published without a
/// reason is said as such, not invented.
pub(crate) fn not_serving_reason(url: &str) -> Option<String> {
    let state = published_state()?;
    if state.listening {
        let dialled = url.trim_end_matches('/');
        let adapter = state
            .adapters
            .into_iter()
            .find(|adapter| format!("http://{}", adapter.bind) == dialled)?;
        let reason = adapter.unserved?;
        return Some(format!(
            "this host's resolver (pid {}) does not serve {} for {}/{}: {reason}",
            state.pid, adapter.bind, adapter.service, adapter.consumer
        ));
    }
    let written = format!(
        "this host's resolver (pid {}, state {} at {}) holds no listener",
        state.pid, state.state, state.updated_at
    );
    Some(
        match state.reason.filter(|reason| !reason.trim().is_empty()) {
            Some(reason) => format!("{written}: {reason}"),
            None => format!("{written}, and published no reason for it"),
        },
    )
}

/// `STADO_RESOLVER_STATE`, the published state, the pid that wrote it, when
/// (epoch) and whether that pid holds its listeners bound (`1`/`0`), for the
/// host-side question whether a resolver role took over its old unit's ports;
/// `None` when no resolver has published.
pub(crate) fn readiness_marker() -> Option<String> {
    let state = published_state()?;
    let written = chrono::DateTime::parse_from_rfc3339(&state.updated_at).ok()?;
    Some(format!(
        "STADO_RESOLVER_STATE\t{}\t{}\t{}\t{}",
        state.state,
        state.pid,
        written.timestamp(),
        u8::from(state.listening)
    ))
}
