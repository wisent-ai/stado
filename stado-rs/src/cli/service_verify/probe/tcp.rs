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
///
/// A connect macOS refuses because this binary may not reach the local
/// network reads as "No route to host", the same words a host that is down
/// gives; the endpoint's addresses are resolved first so that refusal is
/// named by the decision macOS logged for the binary (`remote::lan`), not
/// filed as a service nobody answers for.
pub(super) async fn probe_tcp(endpoint: &str) -> (&'static str, String) {
    let Some((host, port)) = socket_address(endpoint) else {
        return (
            UNVERIFIED,
            format!("endpoint is not a host:port address: {endpoint}"),
        );
    };
    let address = match host.contains(':') {
        true => format!("[{host}]:{port}"),
        false => format!("{host}:{port}"),
    };
    let addresses = match crate::remote::lan::socket_addrs(&host, port).await {
        Ok(addresses) => addresses,
        Err(cause) => return (UNREACHABLE, cause),
    };
    let connected = crate::wait::until(
        crate::wait::Kind::Network,
        "a TCP connection to the declared endpoint",
        address.as_str(),
        tokio::net::TcpStream::connect(&addresses[..]),
    )
    .await;
    match connected {
        Ok(_stream) => (OBSERVED, format!("connected to {address}")),
        Err(error) => match crate::remote::lan::local_network(&error, &addresses).await {
            Some(denied) => (UNREACHABLE, format!("{}: {denied}", root_cause(&error))),
            None => (UNREACHABLE, root_cause(&error)),
        },
    }
}

/// The host and port of a declared endpoint, in whichever form it is written;
/// an IPv6 host without its brackets.
///
/// The service-directory contract requires an origin URL
/// (`http://127.0.0.1:8895`), but a `tcp` endpoint carries no obligation to be
/// spelled with a scheme. An address this cannot resolve is `None`, never a
/// guess: filling in a default port would probe a process the declaration
/// never named and report the answer against a service that has nothing to do
/// with it.
fn socket_address(endpoint: &str) -> Option<(String, u16)> {
    if let Ok(parsed) = url::Url::parse(endpoint) {
        if let (Some(host), Some(port)) = (parsed.host(), parsed.port_or_known_default()) {
            return Some((host.to_string().trim_matches(['[', ']']).to_string(), port));
        }
    }
    let (host, port) = endpoint.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}
