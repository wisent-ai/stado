use std::time::Duration;

use russh::{client, ChannelStream};

use crate::targets::SshConnectionPath;

use crate::deploy::host_access::native::{self, Session};

/// Existing cold-transport allowance used by registry diagnostics.
pub const TUNNEL_OPEN_BUDGET: Duration = Duration::from_secs(30);

/// A native SSH connection shared by requests, without a child or local forward.
pub(crate) struct Tunnel {
    session: Session,
    destination: String,
}

impl Tunnel {
    pub(crate) async fn open(paths: &[SshConnectionPath]) -> Result<Self, String> {
        if paths.is_empty() {
            return Err("active host has no registry SSH connection path".to_string());
        }
        let mut failures = Vec::new();
        for path in paths {
            match native::connect(&path.destination).await {
                Ok(session) => {
                    return Ok(Self {
                        session,
                        destination: path.destination.clone(),
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
        let channel = self
            .session
            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0)
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
}
