//! A socket that speaks no HTTP: is anything accepting on the address?

use crate::observations::{OBSERVED, UNREACHABLE, UNVERIFIED};

use crate::cli::service_verify::probe::root_cause;

/// Connect, then hang up.
///
/// For an endpoint that speaks no HTTP: a database socket, a line protocol, a
/// port whose first byte is the server's. A completed handshake is the whole
/// of what such a declaration promises -- something is accepting on the
/// address the directory hands out -- and nothing is sent, because a probe
/// that guessed at the protocol would at best hang in someone's parser and at
/// worst be a write.
pub(super) async fn probe_tcp(endpoint: &str) -> (&'static str, String) {
    let Some(address) = socket_address(endpoint) else {
        return (
            UNVERIFIED,
            format!("endpoint is not a host:port address: {endpoint}"),
        );
    };
    match tokio::net::TcpStream::connect(address.as_str()).await {
        Ok(_stream) => (OBSERVED, format!("connected to {address}")),
        Err(error) => (UNREACHABLE, root_cause(&error)),
    }
}

/// `host:port` for a declared endpoint, in whichever form it is written.
///
/// The service-directory contract requires an origin URL
/// (`http://127.0.0.1:8895`), but a `tcp` endpoint carries no obligation to be
/// spelled with a scheme. An address this cannot resolve is `None`, never a
/// guess: filling in a default port would probe a process the declaration
/// never named and report the answer against a service that has nothing to do
/// with it.
fn socket_address(endpoint: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(endpoint) {
        if let (Some(host), Some(port)) = (parsed.host(), parsed.port_or_known_default()) {
            return Some(format!("{host}:{port}"));
        }
    }
    let (host, port) = endpoint.rsplit_once(':')?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return None;
    }
    Some(endpoint.to_string())
}
