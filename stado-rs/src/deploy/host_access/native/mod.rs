//! Host-owned SSH sessions; no executable, control master or detached forward.

use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use russh::{client, keys::PublicKeyOrCertificate};

mod auth;
mod config;
mod reverse;
mod session;
mod trust;

pub(crate) use reverse::ReverseForward;
pub(crate) use session::Session;

pub(crate) struct Peer {
    host: String,
    home: PathBuf,
    reverse: Option<reverse::Ports>,
}

impl client::Handler for Peer {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        trust::verify(&self.home, &self.host, key)?;
        Ok(true)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        if let Some(ports) = self.reverse {
            reverse::relay(ports, connected_address, connected_port, channel, reply);
        }
        Ok(())
    }
}

pub(crate) async fn connect(destination: &str) -> Result<Session> {
    connect_with(destination, None).await
}

async fn connect_with(destination: &str, reverse: Option<reverse::Ports>) -> Result<Session> {
    crate::deploy::host_users::validate_ssh_target(destination)?;
    let (user, host) = match destination.split_once('@') {
        Some((user, host)) => (user.to_string(), host),
        None => {
            let user = nix::unistd::User::from_uid(nix::unistd::Uid::current())?
                .context("the current UID has no SSH login name")?;
            (user.name, destination)
        }
    };
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    ensure!(
        !user.is_empty() && !host.is_empty() && !host.contains('@'),
        "invalid SSH destination {destination}"
    );
    if host.contains(':') {
        host.parse::<std::net::Ipv6Addr>()
            .with_context(|| format!("invalid SSH IPv6 destination {destination}"))?;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required for SSH trust and credentials")?;
    let peer = Peer {
        host: host.to_ascii_lowercase(),
        home: home.clone(),
        reverse,
    };
    let handle = client::connect(
        config::client(reverse.is_some()),
        (host, config::SSH_PORT),
        peer,
    )
    .await
    .with_context(|| format!("connect and verify SSH host {destination}"))?;
    let mut session = Session::new(handle);
    let identity = if reverse.is_none() {
        auth::configured_identity(&home)
    } else {
        None
    };
    auth::authenticate(&mut session, &user, &home, identity.as_deref())
        .await
        .with_context(|| format!("authenticate SSH destination {destination}"))?;
    Ok(session)
}
