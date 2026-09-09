//! The listener itself: the [`Dashboard`] every route hangs off, the
//! `--enrollment-only` allowlist that decides what this process publishes at
//! all, and `serve`. The pieces are the HTTP server and request plumbing
//! ([`http`]), the authorization boundaries ([`boundary`]), who a request is
//! allowed to be ([`auth`]), and the routes themselves ([`routes`]). The
//! re-exports below are the shared vocabulary every one of those is written
//! against.

mod auth;
mod boundary;
mod http;
mod routes;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use tokio::sync::Mutex as AsyncMutex;

use crate::config;
use crate::dashboard::{fleet_join, DashboardError};
use crate::queue::{JobStorage, StorageError};
use crate::rate_limit::RateLimiter;

use auth::CachedObjectToken;
use boundary::BoundaryAvailability;

pub use boundary::BOUNDARY_TIMEOUT_OVERRIDE_PATH;

pub(crate) use auth::constant_time_eq;
pub(crate) use boundary::Boundary;
pub(crate) use http::{
    dashboard_error_response, empty_response, http_status, parse_qs, query_value, send_json,
    storage_error_response, trusted_request_host, Request, Response,
};

/// The exact (method, path) pairs `--enrollment-only` serves.
///
/// This is an ALLOWLIST, and it must stay one. A denylist of the sensitive
/// surfaces (`/api/object`, `/api/machine/submit`, ...) goes stale the
/// moment somebody adds a route: the new route is published by
/// default, and the mistake is invisible until the wrong thing answers on a
/// public tunnel. With an allowlist a new route is unreachable in this mode
/// until it is named here on purpose, so forgetting fails closed.
pub(crate) const ENROLLMENT_ROUTES: [(&str, &str); 3] = [
    ("GET", "/join.sh"),
    ("GET", "/api/fleet/invite/key"),
    ("POST", "/api/fleet/join"),
];

/// The machine-side bootstrap script this build serves at `GET /join.sh`.
///
/// Published for `stado fleet ingress`, which verifies a newly opened tunnel
/// by fetching that route from the internet and comparing the bytes with what
/// the listener behind the tunnel would have served. Empty in a build whose
/// source tree had no `deploy/join.sh`, exactly as the route is.
pub fn join_script_source() -> &'static str {
    fleet_join::join_script_source()
}

/// The one body every refused request gets in `--enrollment-only` mode.
///
/// Uniform and mute on purpose: it names no route, no surface and no
/// credential, so a caller cannot learn from a refusal that an operator plane
/// exists elsewhere, nor tell "wrong method on a served path" from "path this
/// listener has never heard of".
pub(crate) const ENROLLMENT_REFUSAL: &[u8] = b"not found\n";

/// Whether `method`+`path` is one of the three enrollment pairs. The query
/// string is not part of the decision; the routes parse their own.
pub(crate) fn enrollment_route_allowed(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or("");
    ENROLLMENT_ROUTES
        .iter()
        .any(|(allowed_method, allowed_path)| *allowed_method == method && *allowed_path == path)
}

#[derive(Clone)]
pub struct Dashboard {
    pub(crate) store: JobStorage,

    pub(crate) rate_limiter: RateLimiter,
    /// Every boundary's live verdict. Written by startup validation and by
    /// the inline recheck a request runs when it finds its boundary closed.
    pub(crate) boundaries: Arc<RwLock<BoundaryAvailability>>,
    /// Namespace bearer cache. Object traffic must not turn into one Skarbiec
    /// read per object request: that exhausted the broker's request capacity
    /// and made the whole object plane answer 503. One async lock also folds a
    /// cold-start burst into one vault read.
    pub(crate) object_tokens: Arc<AsyncMutex<BTreeMap<String, CachedObjectToken>>>,
    /// Release publisher bearers remain usable through a transient Skarbiec
    /// read failure after the release verifier has already proved them. Unlike
    /// object traffic, release traffic refreshes on every request so a token
    /// rotation takes effect immediately; this map is only the bounded
    /// last-known-good fallback.
    pub(crate) release_tokens: Arc<AsyncMutex<BTreeMap<String, CachedObjectToken>>>,
    /// Serve only [`ENROLLMENT_ROUTES`]; every other request is refused
    /// before authorization, before the store and before the vault.
    pub(crate) enrollment_only: bool,
}

impl Dashboard {
    /// Bind the listener to a storage facade.
    pub fn new(store: JobStorage) -> Self {
        Self {
            rate_limiter: RateLimiter::new(store.clone()),
            boundaries: Arc::new(RwLock::new(BoundaryAvailability::default())),
            object_tokens: Arc::new(AsyncMutex::new(BTreeMap::new())),
            release_tokens: Arc::new(AsyncMutex::new(BTreeMap::new())),
            store,
            enrollment_only: false,
        }
    }

    /// Serve only the enrollment routes (`stado dashboard
    /// --enrollment-only`), so this listener can be published through a
    /// tunnel without publishing anything else.
    pub fn with_enrollment_only(mut self, enrollment_only: bool) -> Self {
        self.enrollment_only = enrollment_only;
        self
    }

    pub(crate) fn storage_write_guard(&self) -> Result<Option<std::fs::File>, StorageError> {
        match self.store.local_storage_path() {
            Some(root) => {
                crate::queue::LocalBackend::write_guard_for_root(std::path::Path::new(root))
            }
            None => Ok(None),
        }
    }
}

/// Python `serve(host=None, port=None)`: run the API listener. Blocks until
/// killed. Defaults from `config::dashboard_bind()` /
/// `config::dashboard_port()`; storage from `config::bucket()`.
///
/// `enrollment_only` narrows the listener to `ENROLLMENT_ROUTES` — the mode
/// that is safe to publish through a tunnel.
pub async fn serve(
    host: Option<&str>,
    port: Option<i64>,
    enrollment_only: bool,
) -> Result<(), DashboardError> {
    let host = host
        .map(str::to_string)
        .unwrap_or_else(|| config::dashboard_bind().to_string());
    let port = port.unwrap_or_else(config::dashboard_port);
    let port = u16::try_from(port)
        .map_err(|_| DashboardError::Other(format!("dashboard port out of range: {port}")))?;
    let store = JobStorage::for_server().await?;
    Dashboard::new(store)
        .with_enrollment_only(enrollment_only)
        .serve_with(&host, port)
        .await
}
