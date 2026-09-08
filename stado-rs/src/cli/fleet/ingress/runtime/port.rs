//! Ports: the one loopback port the listener is given, proven free before any
//! process exists to want it.

/// Settle on the loopback port the listener will bind.
///
/// Both branches prove the port is free by binding it here and letting go, and
/// that is the whole guard against the one thing `up` must never do: put a
/// public tunnel in front of a port some other service already holds. A
/// requested port that is taken is refused before any process is started, so
/// the refusal costs nothing and leaves nothing behind.
pub fn reserve_port(requested: Option<u16>) -> Result<u16, String> {
    match requested {
        Some(port) => match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(_) => Ok(port),
            Err(exc) => Err(format!(
                "port {port} on 127.0.0.1 is not free ({exc}); ingress refuses to publish a \
                 tunnel in front of a port it did not open, so nothing was started"
            )),
        },
        None => {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .map_err(|exc| format!("no free loopback port could be reserved: {exc}"))?;
            listener
                .local_addr()
                .map(|address| address.port())
                .map_err(|exc| format!("the reserved loopback port has no address: {exc}"))
        }
    }
}
