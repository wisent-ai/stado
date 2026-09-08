//! The control-point diagnosis: what `/join.sh` answered, named refusals for
//! the three things an operator fixes by different means, and the mode those
//! verdicts allow an invite to be issued in.

use serde_json::{json, Value};

use crate::cli::fleet::invite::record::{MODE_OFFLINE, MODE_ONLINE};

pub(in crate::cli::fleet::invite) mod base;

/// How long a control-point probe may take. An invite is minted while somebody
/// waits for the answer, and a checkpoint slower than this is not one the
/// machine's owner can use either.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Machine-readable verdicts of [`probe_checkpoint`]. The three refusals an
/// operator fixes by different means are named separately on purpose: a name
/// with no DNS answer needs a record, a refused connection needs a listener or
/// a tunnel, and a live server that does not know the route needs a newer
/// release.
pub const REASON_OK: &str = "ok";
pub const REASON_UNRESOLVED: &str = "name_does_not_resolve";
pub const REASON_CONNECTION_REFUSED: &str = "connection_refused";
pub const REASON_ROUTE_UNKNOWN: &str = "route_unknown";
pub const REASON_NOT_CONFIGURED: &str = "not_configured";
pub const REASON_FORCED_OFFLINE: &str = "forced_offline";

/// What `invite` found out about the control point before deciding which mode
/// it can honestly offer. `reason` is the verdict a program branches on,
/// `detail` the sentence a human reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub url: String,
    pub probed: bool,
    pub reachable: bool,
    pub reason: &'static str,
    pub detail: String,
}

impl Checkpoint {
    fn refused(url: &str, reason: &'static str, detail: String) -> Self {
        Self {
            url: url.to_string(),
            probed: true,
            reachable: false,
            reason,
            detail,
        }
    }

    /// The mode an invite can be issued in given this verdict. Online is
    /// reachable-only: everything else, including a control point nobody
    /// configured, is offline.
    pub fn mode(&self) -> &'static str {
        if self.reachable {
            MODE_ONLINE
        } else {
            MODE_OFFLINE
        }
    }
}

/// The verdict as a document. Pure.
pub fn checkpoint_document(checkpoint: &Checkpoint) -> Value {
    json!({
        "url": checkpoint.url,
        "probed": checkpoint.probed,
        "reachable": checkpoint.reachable,
        "reason": checkpoint.reason,
        "detail": checkpoint.detail,
    })
}

/// Host and port `/join.sh` would be fetched from. Pure.
pub fn probe_authority(base: &str) -> Result<(String, u16), String> {
    let parsed = url::Url::parse(base).map_err(|exc| exc.to_string())?;
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "the address names no host".to_string())?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("scheme '{}' has no port", parsed.scheme()))?;
    Ok((host.to_string(), port))
}

/// Ask the configured control point for `/join.sh` before anybody is told to
/// fetch it.
///
/// Resolution is asked for on its own, ahead of the request, so a name with no
/// DNS answer is not reported as a refused connection — the client would
/// collapse both into one transport error, and they are not the same problem.
/// Only a 200 counts: a live server answering 404 knows nothing about invites,
/// which is a release older than these routes, not a network fault.
pub async fn probe_checkpoint(base: &str) -> Checkpoint {
    if base.is_empty() {
        return Checkpoint {
            url: String::new(),
            probed: false,
            reachable: false,
            reason: REASON_NOT_CONFIGURED,
            detail: "no control point is configured (STADO_ENROLLMENT_URL / stado config \
                     enrollment.url and STADO_API_URL / stado config api.url are both empty)"
                .to_string(),
        };
    }
    let endpoint = format!("{base}/join.sh");
    let (host, port) = match probe_authority(base) {
        Ok(authority) => authority,
        Err(detail) => {
            return Checkpoint::refused(
                base,
                REASON_UNRESOLVED,
                format!("control point '{base}' is not a usable address ({detail})"),
            );
        }
    };
    let resolved = tokio::task::spawn_blocking({
        let host = host.clone();
        move || {
            std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .map(|addresses| addresses.count())
                .unwrap_or_default()
        }
    })
    .await
    .unwrap_or_default();
    if resolved == 0 {
        return Checkpoint::refused(
            base,
            REASON_UNRESOLVED,
            format!(
                "control point '{host}' does not resolve to any address, so nothing can fetch /join.sh from it"
            ),
        );
    }
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(exc) => {
            return Checkpoint::refused(
                base,
                REASON_CONNECTION_REFUSED,
                format!("this host could not build an HTTP client to probe {endpoint} ({exc})"),
            );
        }
    };
    match client.get(&endpoint).send().await {
        Err(exc) => Checkpoint::refused(
            base,
            REASON_CONNECTION_REFUSED,
            format!(
                "nothing answered at {endpoint} (connection refused or timed out): {}",
                exc.without_url()
            ),
        ),
        Ok(response) if response.status().as_u16() == 200 => Checkpoint {
            url: base.to_string(),
            probed: true,
            reachable: true,
            reason: REASON_OK,
            detail: format!("{endpoint} answered 200"),
        },
        Ok(response) => Checkpoint::refused(
            base,
            REASON_ROUTE_UNKNOWN,
            format!(
                "{endpoint} answered HTTP {}, not 200: the release serving that host is older than the invite routes",
                response.status().as_u16()
            ),
        ),
    }
}
