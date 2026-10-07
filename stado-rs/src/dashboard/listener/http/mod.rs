//! The hand-rolled minimal HTTP/1.1 server and its request plumbing: the
//! bind and boundary sweep, the accept loop, one connection's request/response
//! cycle, and the [`request`], [`response`], [`query`] and [`host_guard`]
//! pieces every route is written against.

mod host_guard;
mod query;
mod request;
mod response;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::dashboard::DashboardError;

use super::boundary::Boundary;
use super::{
    enrollment_route_allowed, Dashboard, PreparedListener, ENROLLMENT_REFUSAL, ENROLLMENT_ROUTES,
};

pub(crate) use host_guard::{trusted_request_host, valid_beacon_host};
pub(crate) use query::{parse_qs, query_value, strict_url_decode};
pub(crate) use request::{read_request, Request, MAX_HEAD_BYTES};
pub(crate) use response::{
    dashboard_error_response, empty_response, http_status, parse_byte_range, send_json,
    storage_error_response, Response,
};

/// A listener's operating-system failure with the sentence that names the
/// socket, keeping the error's kind so the command states its class.
fn listener_failure(error: std::io::Error, context: String) -> DashboardError {
    DashboardError::Io(std::io::Error::new(
        error.kind(),
        format!("{context}: {error}"),
    ))
}

impl PreparedListener {
    pub(crate) async fn bind(host: &str, port: u16) -> Result<Self, DashboardError> {
        let listener = TcpListener::bind((host, port)).await.map_err(|error| {
            listener_failure(error, format!("could not bind API listener {host}:{port}"))
        })?;
        Self::loopback(listener)
    }

    /// The listening socket this process was given as its standard input.
    pub(crate) fn inherited() -> Result<Self, DashboardError> {
        use std::os::fd::AsFd;
        let descriptor = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| {
                listener_failure(
                    error,
                    "standard input could not be taken as the inherited listener".to_string(),
                )
            })?;
        let listener = std::net::TcpListener::from(descriptor);
        listener.set_nonblocking(true).map_err(|error| {
            listener_failure(
                error,
                "the inherited listener could not be made non-blocking".to_string(),
            )
        })?;
        let listener = TcpListener::from_std(listener).map_err(|error| {
            listener_failure(
                error,
                "standard input is not a listening TCP socket".to_string(),
            )
        })?;
        Self::loopback(listener)
    }

    fn loopback(listener: TcpListener) -> Result<Self, DashboardError> {
        let local_addr = listener.local_addr()?;
        if !local_addr.ip().is_loopback() {
            return Err(DashboardError::Refused(format!(
                "refusing plaintext dashboard bind on non-loopback address {local_addr}; terminate TLS in a loopback reverse proxy"
            )));
        }
        Ok(Self {
            listener,
            local_addr,
        })
    }
}

impl Dashboard {
    pub(crate) async fn serve_prepared(
        &self,
        listener: PreparedListener,
    ) -> Result<(), DashboardError> {
        let PreparedListener {
            listener,
            local_addr,
        } = listener;
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
        // Every verifier reads shared Skarbiec vault/audit state, so the
        // boundaries are validated one after another, each until its verifier
        // answers. A boundary that fails records the verifier's own error and
        // stays closed; the inline recheck in [`Dashboard::recover_boundary`]
        // revalidates it when a request needs it. The listener serves while
        // this runs, so `/healthz` answers and routes stay closed until their
        // own boundary is ready.
        let validation = async {
            for boundary in Boundary::ALL {
                let outcome = self.validate_boundary(boundary).await;
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

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        // Bytes already buffered past the request just served. Reusing one
        // connection is the whole point of this loop, so they have to survive
        // into the next read rather than be dropped with the buffer.
        let mut carry: Vec<u8> = Vec::new();
        loop {
            // A request this listener refuses to read is answered with the
            // reason before the connection closes: dropping it left the client
            // with "connection closed before message completed" and no word
            // about the size or framing that was refused.
            let request = match read_request(&mut stream, &mut carry).await {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    let status = if error.to_string().contains("accepts at most") {
                        reqwest::StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        reqwest::StatusCode::BAD_REQUEST
                    };
                    let mut response = Response::new(
                        http_status(status),
                        status.canonical_reason().unwrap_or_default(),
                        "text/plain; charset=utf-8",
                        error.to_string().as_bytes(),
                    );
                    eprintln!("[dashboard] refused request: {error}");
                    response.close_connection();
                    stream.write_all(&response.bytes).await?;
                    return stream.shutdown().await;
                }
                Err(error) => {
                    eprintln!("[dashboard] request read failed: {error}");
                    return Err(error);
                }
            };
            // The mode gate is the FIRST thing that looks at the request, ahead
            // of the object PUT preflight, ahead of every Host check and
            // authorization, and ahead of any store or vault access. A refused
            // request costs one method/path comparison, and this listener has
            // nothing further to offer it, so the connection closes rather than
            // being held warm for more of the same.
            if self.enrollment_only && !enrollment_route_allowed(&request.method, &request.path) {
                let mut response = Response::new(
                    http_status(reqwest::StatusCode::NOT_FOUND),
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
