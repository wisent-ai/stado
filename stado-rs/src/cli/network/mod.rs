//! `stado network`: this host's place on Stado's own host network
//! ([`crate::network`]). `up` joins it and makes it the host's preferred
//! path, `status` says what it can reach, `down` leaves it; `serve` is the
//! node `up` installs, and `connect` is the stream OpenSSH's `ProxyCommand`
//! runs.

use clap::Subcommand;

use crate::cli::CmdError;

mod down;
mod status;
pub(crate) mod unit;
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

/// `stado network up --host HOST`, as another command runs it for a host it
/// has just brought a Stado to (`stado host upgrade --join-network`).
pub(crate) async fn up_host(host: &str) -> Result<(), CmdError> {
    up::up(up::UpArgs::for_host(host)).await
}

/// Carry standard input to the peer and the peer to standard output, and end
/// when the peer has said everything. Joining both directions kept the proxy
/// until its standard input closed too, so an `ssh` whose remote side had
/// already ended saw no end of the connection from its proxy, and a command
/// run over it stood with nothing running on the host. When standard input
/// ends first, the peer is told (its half is shut down) and its answer is
/// still carried to the end.
async fn connect(host: &str, port: u16) -> Result<(), CmdError> {
    let name = match crate::network::network_host(host) {
        Some(name) => name,
        None => host,
    };
    let stream = crate::network::dial(name, port)
        .await
        .map_err(CmdError::unreachable)?;
    let (mut from_peer, mut to_peer) = tokio::io::split(stream);
    let mut input = standard_input();
    let sending = async move {
        while let Some(chunk) = input.recv().await {
            tokio::io::AsyncWriteExt::write_all(&mut to_peer, &chunk?).await?;
        }
        tokio::io::AsyncWriteExt::shutdown(&mut to_peer).await
    };
    let receiving = async {
        tokio::io::copy(&mut from_peer, &mut tokio::io::stdout()).await?;
        tokio::io::AsyncWriteExt::flush(&mut tokio::io::stdout()).await
    };
    tokio::pin!(sending, receiving);
    let ended = tokio::select! {
        received = &mut receiving => received,
        sent = &mut sending => match sent {
            Ok(()) => (&mut receiving).await,
            Err(error) => Err(error),
        },
    };
    ended.map_err(|error| {
        CmdError::unreachable(format!("the connection to {host}:{port} broke: {error}"))
    })
}

/// This process's standard input, read on a thread of its own and handed
/// over a chunk at a time, ending after its end or its read error. tokio's
/// own standard input reads on the runtime's blocking pool, which the runtime
/// waits for as it shuts down, so a connect whose peer had ended still stood
/// until its standard input closed; a thread of its own ends with the
/// process.
fn standard_input() -> tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (chunks, received) = tokio::sync::mpsc::channel(std::num::NonZeroUsize::MIN.get());
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        let mut input = std::io::stdin().lock();
        loop {
            let chunk = match input.fill_buf() {
                Ok([]) => return,
                Ok(chunk) => Ok(chunk.to_vec()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => Err(error),
            };
            let failed = chunk.is_err();
            if let Ok(read) = &chunk {
                input.consume(read.len());
            }
            if chunks.blocking_send(chunk).is_err() || failed {
                return;
            }
        }
    });
    received
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
