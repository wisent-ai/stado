//! `stado host weles-capture` and `stado host weles-capture-status`: put one
//! batch of `generic_capture` actions on a Weles worker host, then read what
//! the batch produced.
//!
//! The gap this closes: Weles has had the capture primitives all along and
//! exposed none of them, and the only ways into a worker host from here were
//! [`host_exec`](super::host_exec)'s read-only allowlist and a shell nobody is
//! allowed to open. So a plan for 1540 landing-page captures sat in
//! `product-guidelines` with `blockedBy: "no capture action exists on the Weles
//! worker"` written into it, and rendering happened on whichever laptop
//! somebody was sitting at — which is exactly the browser use the workspace
//! forbids off the dedicated host.
//!
//! Three properties this module keeps, because each one was a way the work
//! could have gone wrong:
//!
//! - **The plan is refused before the host is touched.** Every capture is
//!   checked against the contract — schema string, batch id, target, axis,
//!   step vocabulary, artifact prefix — and one bad entry refuses the whole
//!   plan by index. A partially enqueued batch is worse than a rejected one,
//!   because the half that landed still produces artifacts nobody planned.
//! - **The channel is held, not left behind.** `host forward-local` and
//!   `host forward-remote` exist to leave a forward up and write a marker for
//!   it; this one borrows the same option set through
//!   [`host_channel::ssh_options`](super::host_channel::ssh_options) and holds the ssh process for the length of
//!   one command, so nothing survives the call on either side. The remote port
//!   comes from the service directory, never from an operator argument.
//! - **Status needs no memory of the enqueue.** Each action carries its own
//!   `artifact_prefix` in its params, so the state report is assembled from the
//!   worker's action log plus one storage listing. It answers the same on any
//!   control-plane host, including one that never ran the enqueue.

use std::time::Duration;

use serde_json::{Map, Value};

use super::DeployError;

mod channel;
mod diagnostics;
mod plan;
mod receipts;

pub use channel::{
    checked_account_id, latest_action_log, observe_action_payload, open_channel, resolve_admission,
    run_action,
};
pub use diagnostics::{image_diagnostics, run_diagnostic_file, run_diagnostics};
pub use plan::parse_plan;
pub use receipts::{enqueue, status, totals};

/// The plan document's schema string.
///
/// Checked rather than assumed: the field exists so that a JSON document
/// written for something else cannot be enqueued as 1540 browser sessions by
/// accident.
pub const PLAN_SCHEMA: &str = "wisent.weles-capture-plan.v1";

/// The one action this command enqueues, exactly as Weles registers it in its
/// dispatch table and in `weles-action-allowlist.txt`. The admission API
/// refuses any name outside that file, so there is nothing to select here.
pub const CAPTURE_ACTION: &str = "generic_capture";

/// Service-directory key retained for callers; its endpoint is the current
/// synchronous Weles API, not the removed database admission server.
const ADMISSION_SERVICE: &str = "weles-admission";

/// Namespace every capture artifact and sidecar lands in.
pub const ARTIFACT_NAMESPACE: &str = "weles-captures";

/// Credential item carrying the Weles Echo admission API bearer token, for a
/// host whose listener is configured to want one.
///
/// Read through Stado's selected credential store on THIS machine and sent as
/// an `Authorization` header. It is never written to a remote command line: a
/// token in `argv` on the far side is readable by every process on that host.
const ADMISSION_TOKEN_ITEM: &str = "echo-weles-api";
const ADMISSION_TOKEN_FIELD: &str = "token";

const RUN_ROUTE: &str = "/run";
const QUERY_ROUTE: &str = "/v1/echo/action-logs/query";
const QUERY_LIMIT: usize = 5000;
const MAX_STEPS: usize = 100;

/// Weles executes browser trajectories synchronously. Planning and running one
/// complete browser flow may legitimately take tens of minutes, so the worker
/// gets the same 45-minute window as other host-side product executions. The
/// HTTP client stays alive one minute longer so the worker can return its
/// timeout envelope instead of racing the transport deadline.
const RUN_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const REQUEST_DEADLINE: Duration = Duration::from_secs(46 * 60);

/// How long the forward gets to start accepting connections. The ssh connect
/// half is already bounded by the inherited `ConnectTimeout`, so this bounds
/// only the local bind and the remote channel setup.
const FORWARD_DEADLINE: Duration = Duration::from_secs(20);

/// Gap between probes of the forwarded port.
const FORWARD_POLL: Duration = Duration::from_millis(100);

/// The five axes a landing-page capture belongs to. A sixth would be a change
/// to the capture contract, so an unknown one is refused instead of forwarded
/// to a worker that has no route for it.
pub const AXES: [&str; 5] = [
    "composition",
    "interaction",
    "reactivity",
    "state-change",
    "subpage",
];

/// The step vocabulary a capture may ask the worker for.
const STEP_OPS: [&str; 8] = [
    "wait_selector",
    "click",
    "hover",
    "focus",
    "press",
    "scroll",
    "wait_ms",
    "goto",
];

/// Exactly the keys a capture carries. An unexpected key is refused rather
/// than dropped: a plan writer who misspells `full_page` would otherwise get a
/// silently different capture and no way to tell from the report.
const CAPTURE_KEYS: [&str; 9] = [
    "batch",
    "site_slug",
    "source_url",
    "axis",
    "viewport",
    "full_page",
    "steps",
    "record_seconds",
    "artifact_prefix",
];

/// One capture, validated. `params` is the object that goes to the worker
/// verbatim; the named fields are the ones this side reports and matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub site_slug: String,
    pub axis: String,
    pub source_url: String,
    pub artifact_prefix: String,
    pub params: Map<String, Value>,
}

/// A `wisent.weles-capture-plan.v1` document, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub batch: String,
    pub target: String,
    pub captures: Vec<Capture>,
}

/// The character set a batch id may use, identical to the one the forward and
/// release commands accept for a name. The id becomes a storage key segment
/// and a query filter, and both of those are the reason not to widen it.
fn safe_component(kind: &str, value: &str) -> Result<(), DeployError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DeployError(format!(
            "{kind} must contain only letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}
