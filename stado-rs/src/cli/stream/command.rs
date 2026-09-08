//! The clap surface of `stado stream`: one variant per operation, and the
//! flags each one takes.

use clap::Subcommand;

use crate::stream::schema::{DEFAULT_LIBRARY_DIR, DEFAULT_REFRESH_HZ, DEFAULT_RESOLUTION};

fn default_refresh() -> u16 {
    DEFAULT_REFRESH_HZ
}

#[derive(Subcommand, Debug)]
pub enum StreamCommands {
    /// Report what a host could render and encode, without changing it.
    Probe {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Declare that this host carries an interactive session, in the registry.
    Declare {
        target: String,
        /// Virtual screen size the client receives, `WIDTHxHEIGHT`.
        #[arg(long, default_value = DEFAULT_RESOLUTION)]
        resolution: String,
        #[arg(long, default_value_t = default_refresh())]
        refresh_hz: u16,
        /// Driver UUID of the board that renders. Omitted leaves the driver's
        /// default, which is the board the job agent also prefers.
        #[arg(long)]
        gpu_uuid: Option<String>,
        /// Directory for large client data on a volume that has room.
        #[arg(long, default_value = DEFAULT_LIBRARY_DIR)]
        library_dir: String,
        /// Install Steam beside the session.
        #[arg(long)]
        steam: bool,
        /// Pin a Sunshine artifact explicitly, for a distribution this build has
        /// no measured digest for. Requires `--sunshine-sha256`.
        #[arg(long)]
        sunshine_url: Option<String>,
        /// sha256 of that artifact, measured, never guessed.
        #[arg(long)]
        sunshine_sha256: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reconcile the host to its declaration: screen, session, Sunshine, units.
    ///
    /// Retains both native service definitions in the canonical registry, so
    /// later service repairs preserve Xorg ordering and Sunshine's session
    /// preconditions. Openbox diagnostics are retained by the service journal.
    Apply {
        target: String,
        /// Bind the declared library directory onto the host's largest
        /// disk-backed filesystem when it would otherwise land on a root volume
        /// with no room. Without this, such a host is refused and its mounts are
        /// named, because reshaping storage is not something to do quietly.
        #[arg(long)]
        provision_library: bool,
        #[arg(long)]
        json: bool,
    },
    /// What the session is doing right now, and where to point the client.
    Status {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Hand Moonlight's four-digit PIN to Sunshine (no browser involved).
    Pair {
        target: String,
        #[arg(long)]
        pin: String,
        /// Name recorded for the paired client.
        #[arg(long, default_value = "moonlight")]
        client: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop the session. `--purge` also removes the units and the screen.
    Stop {
        target: String,
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        json: bool,
    },
}
