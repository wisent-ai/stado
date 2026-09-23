//! Accept only the requested loopback listener and connect its existing local port.

use russh::{client, Channel};
use tokio::net::TcpStream;

use super::{Ports, LOOPBACK};

pub(in crate::deploy::host_access::native) fn relay(
    ports: Ports,
    connected_address: &str,
    connected_port: u32,
    channel: Channel<client::Msg>,
    reply: client::ChannelOpenHandle,
) {
    if connected_address != LOOPBACK || connected_port != u32::from(ports.remote.get()) {
        eprintln!(
            "stado reverse forward refused unrequested listener {connected_address}:{connected_port}; expected {LOOPBACK}:{}",
            ports.remote
        );
        // Dropping the native reply explicitly rejects this channel.
        return;
    }
    tokio::spawn(async move {
        let mut local = match TcpStream::connect((LOOPBACK, ports.local.get())).await {
            Ok(local) => local,
            Err(error) => {
                eprintln!(
                    "stado reverse forward could not connect local endpoint {LOOPBACK}:{}: {error}",
                    ports.local
                );
                return;
            }
        };
        reply.accept().await;
        let mut remote = channel.into_stream();
        if let Err(error) = tokio::io::copy_bidirectional(&mut remote, &mut local).await {
            eprintln!(
                "stado reverse forward relay {LOOPBACK}:{} -> {LOOPBACK}:{} failed: {error}",
                ports.remote, ports.local
            );
        }
    });
}
