//! `stado network`: this host's place on Stado's own host network
//! ([`crate::network`]). `up` joins it and makes it the host's preferred
//! path, `status` says what it can reach, `down` leaves it; `serve` is the
//! node `up` installs, and `connect` is the stream OpenSSH's `ProxyCommand`
//! runs.

use clap::Subcommand;

use crate::cli::CmdError;

mod down;
mod status;
mod unit;
mod up;

#[derive(Debug, Subcommand)]
pub enum NetworkCommands {
    /// Join a host to Stado's network: mint its key, measure its endpoints,
    /// declare its node in the registry with its network address as the
    /// preferred SSH path, install and start the node, and see it answer.
    /// With `--host` naming another machine, that host's own Stado runs it
    /// over its host channel.
    Up(up::UpArgs),
    /// This host's node and every declared peer: reached directly, through a
    /// relay, or not, with the cause.
    Status {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Leave Stado's network: stop and remove the node, take its declaration
    /// out of the registry and make the host's first alternate path the
    /// preferred one again. The key stays, so `up` restores the same identity.
    Down {
        /// The registry host this is; this machine's own registry entry when
        /// omitted.
        #[arg(long)]
        host: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Run this host's network node in the foreground (what the unit `up`
    /// installs runs).
    Serve,
    /// Carry standard input and output to HOST's port PORT over Stado's
    /// network: OpenSSH's `ProxyCommand=stado network connect %h %p`.
    Connect {
        /// The registry host, or its network name `<host>.stado.internal`.
        host: String,
        /// The SSH port the host's registry paths name.
        port: u16,
    },
}

pub async fn dispatch(command: NetworkCommands) -> Result<(), CmdError> {
    match command {
        NetworkCommands::Up(args) => up::up(args).await,
        NetworkCommands::Status { json } => status::status(json).await,
        NetworkCommands::Down { host, json } => down::down(host.as_deref(), json).await,
        NetworkCommands::Serve => crate::network::node::serve()
            .await
            .map_err(CmdError::unreachable),
        NetworkCommands::Connect { host, port } => connect(&host, port).await,
    }
}

async fn connect(host: &str, port: u16) -> Result<(), CmdError> {
    let name = match crate::network::network_host(host) {
        Some(name) => name,
        None => host,
    };
    let stream = crate::network::dial(name, port)
        .await
        .map_err(CmdError::unreachable)?;
    let (mut from_peer, mut to_peer) = tokio::io::split(stream);
    let sending = async {
        tokio::io::copy(&mut tokio::io::stdin(), &mut to_peer).await?;
        tokio::io::AsyncWriteExt::shutdown(&mut to_peer).await
    };
    let receiving = async {
        tokio::io::copy(&mut from_peer, &mut tokio::io::stdout()).await?;
        tokio::io::AsyncWriteExt::flush(&mut tokio::io::stdout()).await
    };
    // The connection is over when the peer has said everything: OpenSSH
    // ends the proxy after the session, and a half still open on our side
    // only means its standard input was not closed.
    let (_, received) = tokio::join!(sending, receiving);
    received.map_err(|error| {
        CmdError::unreachable(format!("the connection to {host}:{port} broke: {error}"))
    })
}

/// The registry host `host` names when `copy`, this machine's registry read
/// without the authority, tells it apart from this machine: `network up
/// --host` for another host runs on that host. A copy that cannot tell this
/// machine apart answers `None`: the command runs here, where the registry
/// the authority answers settles whether `--host` is this machine.
pub(super) fn another_machine(copy: &crate::targets::Registry, host: &str) -> Option<String> {
    let named = copy.lookup(host)?.name.clone();
    let hostname = crate::providers::vast::system_hostname();
    let this = copy.lookup_self(&hostname).ok()??;
    (this.name != named).then_some(named)
}

/// The registry host this machine is: `--host`, else this machine's own
/// registry entry. A `--host` that names another machine the registry tells
/// apart from this one is refused: the node, its key and its unit are this
/// machine's.
pub(super) fn this_host(
    registry: &crate::targets::Registry,
    host: Option<&str>,
) -> Result<String, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let found = registry.lookup_self(&hostname);
    if let Some(host) = host {
        let named = registry
            .lookup(host)
            .map(|target| target.name.clone())
            .ok_or_else(|| CmdError::missing(format!("the registry declares no host {host}")))?;
        if let Ok(Some(this)) = found {
            if this.name != named {
                return Err(CmdError::refused(format!(
                    "--host {named} is not this machine: the registry identifies this machine ({hostname}) as {}; `stado network` runs on the host it joins",
                    this.name
                )));
            }
        }
        return Ok(named);
    }
    found
        .map_err(|error| CmdError::declaration(format!("this machine ({hostname}) cannot be told apart in the registry: {error}")))?
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            CmdError::missing(format!(
                "no registry host is this machine ({hostname}); `--host NAME` names it, `stado registry host add` declares it"
            ))
        })
}
