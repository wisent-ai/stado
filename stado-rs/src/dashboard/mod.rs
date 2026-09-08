//! The Stado API listener: the authenticated object, release, machine,
//! service, host-health and enrollment HTTP surface. Port of
//! `stado/dashboard.py` (ThreadingHTTPServer) with the HTML operator
//! dashboard removed — the operator workspace is Stado Desktop, and this
//! listener serves no page.
//!

//! GET/PUT/DELETE /api/object?uri=stado://... - product object data plane
//! GET /api/object/list?namespace=...&prefix=... - product object listing
//! GET /api/object/stat?uri=stado://... - product object metadata
//! POST /api/object/compose - atomically publish verified object chunks
//! PUT /api/host-health?host=... - route-scoped authenticated host beacon publication
//! GET /api/host/inventory?target=... - authenticated fresh inventory of one declared host
//! GET/POST /api/host/storage-root-reconcile?target=...&transaction=...&phase=... - durable storage handoff
//! GET /api/release/object?uri=stado://releases/... - public software release download
//! POST /api/machine/submit - submit a canonical machine request
//! GET /api/machine/status?job_id=... - read canonical machine status
//! POST /api/machine/cancel?job_id=... - durably cancel a machine job
//! GET /api/service/status?name=... - read one managed service's beacon status
//! POST /api/service/restart?name=... - restart one managed service on every declared host
//! GET/POST /api/service/converge?target=...[&binary=...] - report or apply host convergence
//! POST /api/rate-limit/consume - authenticated shared atomic rate-limit consume
//! POST /api/registry/import - additive canonical registry adoption
//! POST /api/integration/enterprise/<action> - authenticated fleet projection
//! POST /api/integration/oko/<action> - authenticated finite Oko selected-host dispatch
//! POST /api/operator/run - bounded native Desktop operator actions
//! GET /api/fleet/invite/key - invite-token-authenticated public channel key
//! POST /api/fleet/join - invite-token-authenticated pending enrollment request
//! GET /join.sh           - machine-side enrollment bootstrap script (public)
//! GET /healthz           - liveness (before auth, after the Host guard)
//! GET /livez             - Cloud Run liveness alias
//!
//! `--enrollment-only` narrows this listener to exactly three of the routes
//! above — `GET /join.sh`, `GET /api/fleet/invite/key`,
//! `POST /api/fleet/join` — and answers 404 to every other path and method
//! before authorization, the store or the vault is touched. That mode exists
//! so the enrollment routes can be published through a tunnel without
//! publishing the object, machine or service planes with them. See
//! `ENROLLMENT_ROUTES`.
//!
//! The application plane was extracted into the private `wisent-backend`
//! service; Stado keeps only the generic object plane. The product
//! integrations (Stripe, Resend, SendGrid, GitHub, HuggingFace, captcha
//! proxies) were extracted into the private `wisent-integrations` service.
//!
//! DEVIATIONS from Python (deliberate):
//! - Hand-rolled minimal HTTP/1.1 on `tokio::net::TcpListener`, one task per
//!   accepted connection (the ThreadingHTTPServer equivalent); the port
//!   spec forbids adding a web-framework dependency. Python's implicit
//!   `Server:`/`Date:` response headers and its error-page HTML bodies are
//!   not reproduced (status codes and JSON bodies match).

mod fleet_join;
mod integration;
mod listener;
mod operator_auth;
mod operator_console;
mod registry_policy;

use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::StorageError;

use listener::{constant_time_eq, http_status, send_json, trusted_request_host, Request, Response};

pub use listener::{
    boundaries_without_a_reopening_route, join_script_source, release_coordinate_boundary_split,
    serve, Dashboard, BOUNDARY_TIMEOUT_OVERRIDE_PATH,
};

/// Dashboard serve failure.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// Storage failures from the data-plane routes.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Listener/socket failures.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Port validation and other serve failures.
    #[error("{0}")]
    Other(String),
}
