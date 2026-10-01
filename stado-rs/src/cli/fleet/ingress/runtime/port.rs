//! Ports: the one loopback port the listener is given, bound here before any
//! process exists to want it.

use std::net::TcpListener;

/// Bind the loopback socket the listener will serve.
///
/// The socket stays bound and is handed to the listener as its standard
/// input, so the port this command proved free is the port that serves: no
/// other process can take it in between, and a connection made to it before
/// the listener accepts waits in the socket's backlog instead of being
/// refused. A requested port that is taken is refused before any process is
/// started, so the refusal costs nothing and leaves nothing behind.
pub fn reserve_port(requested: Option<u16>) -> Result<TcpListener, String> {
    match requested {
        Some(port) => TcpListener::bind(("127.0.0.1", port)).map_err(|exc| {
            format!(
                "port {port} on 127.0.0.1 is not free ({exc}); ingress refuses to publish a \
                 tunnel in front of a port it did not open, so nothing was started"
            )
        }),
        None => TcpListener::bind(("127.0.0.1", 0))
            .map_err(|exc| format!("no free loopback port could be bound: {exc}")),
    }
}
