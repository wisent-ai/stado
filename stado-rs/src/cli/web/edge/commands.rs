//! The `stado web edge` subcommands, as clap parses them.

use clap::Subcommand;

use super::{DEFAULT_REGION, DEFAULT_SIZE};

#[derive(Debug, Subcommand)]
pub(crate) enum EdgeCommands {
    /// Create the edge host on Azure and record it as the fleet's edge.
    Provision {
        /// Name for the VM, and the registry target name it is recorded
        /// under: lowercase letters, digits and dashes.
        name: String,
        /// Azure region. It must be one with a pre-provisioned vnet and
        /// subnet; Azure refuses the NIC otherwise, in its own words.
        #[arg(long, default_value = DEFAULT_REGION)]
        region: String,
        /// Azure VM size. The default is ARM64, matching the ARM64 image this
        /// command boots.
        #[arg(long, default_value = DEFAULT_SIZE)]
        size: String,
        /// Address Let's Encrypt sends certificate-expiry mail to.
        #[arg(long)]
        contact: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Record an edge host that already exists, provisioning nothing.
    ///
    /// The path for a host Stado did not create: an operator's own VM, a
    /// colocated box, or an edge provisioned before this command existed.
    Declare {
        /// Registry target name of the edge host.
        #[arg(long)]
        target: String,
        /// Its public IPv4 address, which product hostnames' A records point
        /// at.
        #[arg(long)]
        address: String,
        /// Address Let's Encrypt sends certificate-expiry mail to.
        #[arg(long)]
        contact: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// The declared edge, whether it answers, and what it terminates.
    Status {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Reconcile: the hostnames the edge must terminate against the ones its
    /// proxy currently does.
    Hostnames {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Undo `provision`: delete the edge's Azure resources and forget it.
    ///
    /// The one command that reverses the only thing in this capability that
    /// spends money. It refuses while any product still names the `stado`
    /// edge, because deleting the host those hostnames resolve to is an
    /// outage rather than a cleanup — retract them with `stado web remove`
    /// first, or pass `--orphan-hostnames` to say that is what you mean.
    Remove {
        /// Delete the resources even though products still name this edge.
        #[arg(long)]
        orphan_hostnames: bool,
        /// Forget the declaration without deleting anything on Azure. For an
        /// edge Stado did not create, which it must not delete either.
        #[arg(long)]
        keep_resources: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}
