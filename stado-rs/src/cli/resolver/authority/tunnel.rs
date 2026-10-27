use russh::{client, ChannelStream};

use crate::targets::SshConnectionPath;

use crate::deploy::host_access::native::{self, Session};

/// The originator port a direct-tcpip channel names when no local socket
/// originates it: the field is informational and this channel has none.
/// https://www.rfc-editor.org/rfc/rfc4254#section-7.2 "uint32 port of originator"
const NO_ORIGINATOR_PORT: u32 = 0;

/// A native SSH connection shared by requests, without a child or local
/// forward. It is long-lived, so it is opened only with SSH keepalive
/// declared: a peer that stops answering then ends the session and
/// [`Tunnel::usable`] says so, instead of a channel open waiting on it.
pub(crate) struct Tunnel {
    session: Session,
    destination: String,
    /// When this session answered, so a wait can be told apart as begun on
    /// it or on one it replaced.
    opened: std::time::Instant,
}

impl Tunnel {
    pub(crate) async fn open(paths: &[SshConnectionPath]) -> Result<Self, String> {
        if paths.is_empty() {
            return Err("active host has no registry SSH connection path".to_string());
        }
        let mut failures = Vec::new();
        for path in paths {
            match native::connect_long_lived(&path.destination).await {
                Ok(session) => {
                    return Ok(Self {
                        session,
                        destination: path.destination.clone(),
                        opened: std::time::Instant::now(),
                    })
                }
                Err(error) => failures.push(format!("{}: {error:#}", path.name)),
            }
        }
        Err(format!(
            "no registry SSH connection path answered ({})",
            failures.join("; ")
        ))
    }

    pub(crate) async fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Result<ChannelStream<client::Msg>, String> {
        // The originator address is this host's own loopback, as an
        // informational field (RFC 4254 7.2) with no socket behind it.
        let originator = std::net::Ipv4Addr::LOCALHOST.to_string();
        let channel = self
            .session
            .channel_open_direct_tcpip(host, u32::from(port), &originator, NO_ORIGINATOR_PORT)
            .await
            .map_err(|error| {
                format!(
                    "SSH destination {} refused channel to {host}:{port}: {error}",
                    self.destination
                )
            })?;
        Ok(channel.into_stream())
    }

    pub(crate) fn usable(&self) -> bool {
        !self.session.is_closed()
    }

    /// How long ago this session answered.
    pub(crate) fn age(&self) -> std::time::Duration {
        self.opened.elapsed()
    }
}
