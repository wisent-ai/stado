//! The hand-rolled minimal HTTP/1.1 server and its request plumbing: the
//! bind and boundary sweep, the accept loop, one connection's request/response
//! cycle, and the [`request`], [`response`], [`query`] and [`host_guard`]
//! pieces every route is written against.

mod host_guard;
mod query;
mod request;
mod response;

use futures::stream::{FuturesUnordered, StreamExt};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::dashboard::DashboardError;

use super::boundary::Boundary;
use super::{enrollment_route_allowed, Dashboard, ENROLLMENT_REFUSAL, ENROLLMENT_ROUTES};

pub(crate) use host_guard::{trusted_request_host, valid_beacon_host};
pub(crate) use query::{parse_qs, query_value, strict_url_decode};
pub(crate) use request::{read_request, Request, MAX_HEAD_BYTES};
pub(crate) use response::{
    dashboard_error_response, empty_response, http_status, parse_byte_range, send_json,
    storage_error_response, Response,
};

impl Dashboard {
    /// Start the boundary checks and serve HTTP on loopback. This server does
    /// not terminate TLS, so binding it to a non-loopback interface would
    /// expose bearer-authenticated routes over plaintext. Production ingress
    /// must terminate TLS in a reverse proxy and forward to this listener.
    pub async fn serve_with(&self, host: &str, port: u16) -> Result<(), DashboardError> {
        let listener = TcpListener::bind((host, port)).await?;
        let local_addr = listener.local_addr()?;
        if !local_addr.ip().is_loopback() {
            return Err(DashboardError::Other(format!(
                "refusing plaintext dashboard bind on non-loopback address {local_addr}; terminate TLS in a loopback reverse proxy"
            )));
        }
        if self.enrollment_only {
            // Nothing below this branch is started, because nothing below it
            // is reachable in this mode:
            //
            // * the seven Skarbiec boundary verifiers only gate object,
            //   release, machine, service, rate-limit and integration routes,
            //   all of which are refused by the allowlist. Skipping them also
            //   means this listener needs no vault at all, which is the point:
            //   it can run where the operator plane cannot.
            //
            // `boundaries` therefore stays all-false; no served route reads
            // it.
            //
            // This log is the operator's only confirmation of what they are
            // about to publish, so it names every served pair verbatim.
            eprintln!("[dashboard] enrollment-only listener on http://{local_addr}");
            eprintln!(
                "[dashboard] this listener serves ONLY the enrollment routes; every other path and method answers 404:"
            );
            for (method, path) in ENROLLMENT_ROUTES {
                eprintln!("[dashboard]   {method} {path}");
            }
            eprintln!(
                "[dashboard] no object, machine, service, host-health or integration route is served here"
            );
            return self.serve_on(listener).await;
        }
        // Every verifier reads shared Skarbiec vault/audit state. Starting all
        // boundaries together can overwhelm the listener and fail the whole
        // control plane on a transient connection reset, so validate them in
        // deterministic order with an independent timeout per boundary
        // ([`boundary_timeout`]).
        //
        // A verdict used to be recorded once and never revisited, so one slow
        // or reset read shut a boundary until somebody restarted the unit --
        // and `object` shutting means `503 object authorization unavailable`
        // for the whole fleet. That happened four times in one afternoon, each
        // time cured by an identical retry, so the retry belongs here instead
        // of in the operator's hands. The eager sweep below is that retry; the
        // inline recheck in [`Dashboard::recover_boundary`] is the other half,
        // because a boundary that resets an hour after startup never reaches
        // this code again.
        // Do not hold the listener behind this sweep. A slow upstream used to
        // leave the socket bound but unserved for minutes, so launchd and every
        // recovery client saw a timeout instead of the available `/healthz`
        // report. Routes remain closed until their own boundary is ready and
        // can revalidate it inline through `boundaries_available`.
        let validation = async {
            let attempts = std::env::var("WC_DASHBOARD_BOUNDARY_ATTEMPTS")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|count| *count > 0)
                .unwrap_or(3);
            let retry_pause = Duration::from_secs(2);
            for boundary in Boundary::ALL {
                let mut outcome = self.validate_boundary(boundary).await;
                let mut attempt = 1;
                while attempt < attempts && outcome.is_err() {
                    eprintln!(
                    "[dashboard] {} boundary attempt {attempt} of {attempts} did not settle; retrying",
                    boundary.label()
                );
                    tokio::time::sleep(retry_pause).await;
                    outcome = self.validate_boundary(boundary).await;
                    attempt += 1;
                }
                // Only `object` used to report why it failed, so every other
                // boundary said "unavailable" and left the operator guessing which
                // grant, item set or endpoint was at fault. The verdict is useless
                // without the reason, so the log carries the verifier's own words.
                if let Err(error) = &outcome {
                    eprintln!("[dashboard] {} boundary error: {error}", boundary.label());
                    eprintln!("[dashboard] {} boundary unavailable", boundary.label());
                }
                self.record_boundary(boundary, outcome);
            }
        };

        eprintln!("[dashboard] listening on http://{local_addr}");
        let serving = self.serve_on(listener);
        tokio::pin!(validation);
        tokio::pin!(serving);
        tokio::select! {
            result = &mut serving => result,
            _ = &mut validation => serving.await,
        }
    }

    /// Accept loop on an already-bound listener (tests bind 127.0.0.1:0).
    /// One task per connection — the ThreadingHTTPServer equivalent.
    pub async fn serve_on(&self, listener: TcpListener) -> Result<(), DashboardError> {
        let mut connections = FuturesUnordered::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let dashboard = self.clone();
                    connections.push(async move {
                        if let Err(exc) = dashboard.handle_connection(stream).await {
                            eprintln!("[dashboard] connection error: {exc}");
                        }
                    });
                }
                _ = connections.next(), if !connections.is_empty() => {}
            }
        }
    }

    /// How long this side waits for a request head before it closes the socket.
    ///
    /// It bounds the first request as much as a reused one: a connection that
    /// is opened and then abandoned holds a task and a file descriptor exactly
    /// like an idle reused one, and the accept loop puts no bound on how many
    /// of those may exist. Only the head is bounded, never the body -- one
    /// object PUT may declare up to `max_object_bytes`, and a slow upload is
    /// progress rather than idleness.
    ///
    /// It must stay strictly LONGER than the object client's pool idle timeout
    /// (90 s: reqwest's default, made explicit alongside the keyed client).
    /// Equal timers race -- the client takes a warm connection out of its pool
    /// in the same instant this side sends FIN, and the request written into it
    /// then fails or re-dials, which is the cost this change exists to remove.
    /// With the client retiring first, the socket is always closed by the side
    /// that is not about to write to it.
    const KEEP_ALIVE_IDLE: std::time::Duration = std::time::Duration::from_secs(120);

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        // The peer is the reverse proxy's loopback address behind an HTTPS
        // ingress; the invite routes only ever use it as a rate-limit bucket.
        let peer = stream.peer_addr().ok().map(|address| address.ip());
        // Bytes already buffered past the request just served. Reusing one
        // connection is the whole point of this loop, so they have to survive
        // into the next read rather than be dropped with the buffer.
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let Some(mut request) =
                read_request(&mut stream, &mut carry, Self::KEEP_ALIVE_IDLE).await?
            else {
                return Ok(());
            };
            request.peer = peer;
            // The mode gate is the FIRST thing that looks at the request, ahead
            // of the object PUT preflight, ahead of every Host check and
            // authorization, and ahead of any store or vault access. A refused
            // request costs one method/path comparison, and this listener has
            // nothing further to offer it, so the connection closes rather than
            // being held warm for more of the same.
            if self.enrollment_only && !enrollment_route_allowed(&request.method, &request.path) {
                let mut response = Response::new(
                    http_status("404"),
                    "Not Found",
                    "text/plain; charset=utf-8",
                    ENROLLMENT_REFUSAL,
                );
                eprintln!(
                    "[dashboard] \"{} {} HTTP/1.1\" {} enrollment-only",
                    request.method, request.path, response.status
                );
                response.close_connection();
                stream.write_all(&response.bytes).await?;
                return stream.shutdown().await;
            }
            if request.method == "PUT" && request.path.starts_with("/api/object?") {
                // A preflight answer refuses the write outright, so this
                // connection has nothing left to serve either.
                if let Some(mut response) = self.object_put_preflight(&request).await {
                    response.close_connection();
                    stream.write_all(&response.bytes).await?;
                    return stream.shutdown().await;
                }
            }
            // `read_request` hands over a body of exactly `content_length`, so
            // no request can leave a byte behind to be read as the head of the
            // next one; the carried remainder is the next request already.
            //
            // No route serves HEAD, so a HEAD would be answered with the body a
            // GET returns. A client that correctly reads no body after a HEAD
            // would parse those bytes as its next response, so HEAD ends the
            // connection instead of poisoning it.
            let keep_alive = request.wants_keep_alive() && request.method != "HEAD";
            let mut response = self.route(&request).await;
            eprintln!(
                "[dashboard] \"{} {} HTTP/1.1\" {} -",
                request.method, request.path, response.status
            );
            if !keep_alive {
                response.close_connection();
            }
            stream.write_all(&response.bytes).await?;
            if response.status == 101 {
                return crate::dashboard::operator_console::stream::serve(stream, carry).await;
            }
            if !keep_alive {
                return stream.shutdown().await;
            }
        }
    }
}
