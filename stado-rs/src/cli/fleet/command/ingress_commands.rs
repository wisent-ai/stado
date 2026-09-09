//! The `stado fleet ingress` command tree: the published parser surface.

use clap::Subcommand;

/// The three things an operator does to the fleet's public entrance. There is
/// no `restart`: a quick tunnel comes back under a different address, so the
/// operation that reads like "the same entrance again" is exactly the one that
/// silently invalidates every invitation already handed out. `down` then `up`
/// says what happened.
#[derive(Subcommand)]
pub enum IngressCommands {
    /// Start the narrow enrollment listener and a tunnel in front of it,
    /// verify the public address from the internet, then publish it.
    Up {
        /// Loopback port for the listener. Chosen automatically when omitted;
        /// a port already in use is refused, never adopted.
        #[arg(long)]
        port: Option<u16>,
        /// Use a named tunnel on the fleet's own domain instead of a quick
        /// one. Refused today: the Cloudflare API token it needs does not
        /// exist in the vault.
        #[arg(long)]
        named: bool,
    },
    /// What is published, whether it still answers, and how old it is.
    Status {
        /// Emit the machine-readable document instead of the report.
        #[arg(long)]
        json: bool,
    },
    /// Close the tunnel, stop the listener, and unpublish the address.
    Down,
}
