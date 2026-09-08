//! Selection and deadlines: which probes a scope runs, and how long each one
//! is allowed to take.

use std::future::Future;
use std::time::Duration;

use crate::doctor::{Check, RunScope, PROBE_TIMEOUT};

/// Run a probe under an explicit deadline. A row whose work grows with the
/// deployment cannot share one flat budget with a row that makes a single
/// call: the gateway sweep reads every mapped item through one listener, and
/// under the flat bound it failed intermittently with "probe did not answer"
/// while the same sweep measured under a second.
async fn bounded_within(
    deadline: Duration,
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    match tokio::time::timeout(deadline, probe).await {
        Ok(check) => check,
        // NOT a FAIL. The budget elapsed, which says the probe did not
        // answer in time and says nothing whatever about the deployment.
        // Reported as a failure it made three checks read broken under the
        // doctor's own load and pass again minutes later, which teaches an
        // operator to discount every row in the table.
        Err(_) => Check::unmeasured(
            id,
            title,
            format!(
                "not measured: the probe did not answer within {deadline:?}, so this check \
                 says nothing about the deployment either way"
            ),
            remedy,
        ),
    }
}

/// Run a selected probe under the shared deadline. An unselected future is
/// dropped without being polled, so scoped doctor modes do not load unrelated
/// dependencies.
pub(super) async fn selected(
    scope: RunScope,
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    selected_within(scope, PROBE_TIMEOUT, id, title, remedy, probe).await
}

pub(super) async fn selected_within(
    scope: RunScope,
    deadline: Duration,
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    if !scope.includes(id) {
        return Check::pass(id, title, String::new(), remedy);
    }
    bounded_within(deadline, id, title, remedy, probe).await
}

/// The storage probe performs write, read, and unconditional cleanup
/// sequentially. Give each network operation one ordinary probe allowance plus
/// one allowance for scheduling behind the other preflight storage readers.
pub(super) fn storage_round_trip_deadline() -> Duration {
    PROBE_TIMEOUT * 4
}

/// The registry read crosses the resolver's forward, and a forward that has
/// gone cold is declared to need [`crate::cli::resolver::TUNNEL_OPEN_BUDGET`]
/// before it accepts its first connection. Bounding this probe at the flat
/// allowance measured the channel's cold start instead of the registry: on
/// 2026-09-03 `stado registry pull` answered in 5.4s on a warm channel and
/// 35.9s after seventy seconds of idling, so `registry FAIL probe did not
/// answer within 8s` was the verdict on a registry that was answering, and it
/// failed the 0.13.52 deployment preflight and with it the release.
///
/// So the budget is the transport's declared establishment allowance plus one
/// ordinary probe allowance for the read itself, and the numbers come from the
/// two declarations rather than from a third one written here.
pub(super) fn registry_probe_deadline() -> Duration {
    crate::cli::resolver::TUNNEL_OPEN_BUDGET.saturating_add(PROBE_TIMEOUT)
}

/// Per-item allowance for the gateway-auth sweep, multiplied by the number of
/// items the four verifier mappings actually declare.
pub(super) fn object_auth_deadline() -> Duration {
    // Each mapping resolves independently; one that cannot be read contributes
    // nothing to the sweep, so it contributes nothing to the budget either.
    let mapped = crate::config::object_api_namespaces().map_or(usize::MIN, |items| items.len())
        + crate::config::release_api_publishers().map_or(usize::MIN, |items| items.len())
        + crate::config::machine_api_clients().map_or(usize::MIN, |items| items.len())
        + crate::config::service_api_deployers().map_or(usize::MIN, |items| items.len());
    PROBE_TIMEOUT + PROBE_TIMEOUT * u32::try_from(mapped).unwrap_or_default()
}

/// The agent lists its own grant through the host's Skarbiec, which decrypts
/// with GnuPG on one listener — the same one the gateway-auth sweep above is
/// reading forty-eight mapped items through at that moment.
///
/// Bounding that at the flat allowance measured the queue rather than the
/// broker: on 2026-09-05, with `agent.skarbiec.url` finally pointing at the
/// endpoint the directory declares, five sequential reads of that grant took
/// 1.6s, 2.1s, 2.9s, 4.7s and 14.4s, and the check reported `not measured:
/// the probe did not answer within 8s` twice in a row about a broker that was
/// answering every request with 200. So the allowance is one probe for the
/// read and one for waiting behind the concurrent sweep. A broker that has
/// genuinely stopped answering — the 60-second stalls the same host produced
/// before `skarbiec recover-daemons` — still elapses, and still reports
/// unmeasured rather than a verdict.
pub(super) fn agent_skarbiec_deadline() -> Duration {
    PROBE_TIMEOUT * 2
}
/// Allowance for resolving alert channels. Each enabled channel reads its own
/// destination and provider material out of the vault, through the same
/// single-threaded listener the gateway sweep is using at the same moment —
/// and the resolved channel is then asked of its provider over the internet.
///
/// The provider round trip was never in this budget, and with one declared
/// channel the whole check had 16 seconds for a vault read plus an HTTPS
/// exchange. It elapsed twice on 2026-09-02, at 19:31:04 and 19:46:16, and
/// each time a timed-out probe was rendered as `alerts FAIL` under the remedy
/// "configure at least one non-GCP channel" — which blocked a release
/// delivery over a channel that was configured and, measured two minutes
/// later, working. One deadline per declared channel plus one for the
/// provider exchange keeps the bound honest and still bounded.
pub(super) fn alerts_deadline() -> Duration {
    let channels = crate::config::alert_channels().len();
    PROBE_TIMEOUT * 2 + PROBE_TIMEOUT * u32::try_from(channels).unwrap_or_default()
}
