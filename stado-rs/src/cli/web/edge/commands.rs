//! The `stado web edge` subcommands, as clap parses them.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum EdgeCommands {
    /// Create the edge host on Azure and record it as the fleet's edge.
    Provision {
        /// Name for the VM, and the registry target name it is recorded
        /// under: lowercase letters, digits and dashes.
        name: String,
        /// Azure region. It must be one with a pre-provisioned vnet and
        /// subnet; Azure refuses the NIC otherwise, in its own words.
        #[arg(long)]
        region: String,
        /// Azure VM size. It must match `--image`'s architecture: Azure
        /// refuses an ARM64 image on an x86-64 size, and that refusal is
        /// passed through word for word.
        #[arg(long)]
        size: String,
        /// Image URN the edge boots, `publisher:offer:sku:version`.
        #[arg(long)]
        image: String,
        /// OS disk in GiB. It holds the proxy binary, its generated
        /// configuration and Caddy's certificate store.
        #[arg(long)]
        disk_gb: i64,
        /// Address Let's Encrypt sends certificate-expiry mail to.
        #[arg(long)]
        contact: String,
        /// Seconds `edge status` waits for 80 and 443 to answer from the
        /// machine it runs on before reporting no answer.
        #[arg(long)]
        probe_seconds: u64,
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
        /// Skarbiec item holding the registrar's api_user, api_key, username
        /// and client_ip; `stado web route` writes the A records through it.
        /// Kept from the earlier declaration when omitted.
        #[arg(long)]
        registrar_credential: Option<String>,
        /// Seconds `edge status` waits for 80 and 443 to answer before
        /// reporting no answer. Kept from the earlier declaration when
        /// omitted; `edge status` refuses an edge that declares none.
        #[arg(long)]
        probe_seconds: Option<u64>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// The declared edge, every security group between the Internet and it,
    /// whether its ports answer, and what it terminates.
    Status {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Open 80 and 443 in every security group between the Internet and
    /// the edge that drops them, to the edge's own private address only.
    ///
    /// Azure applies the subnet's group before the interface's and each
    /// must allow a connection; a group with no allow ends in its default
    /// deny. The allow names the edge interface's address, never the subnet,
    /// so no other machine of the subnet is opened. A group outside
    /// AZURE_RESOURCE_GROUP is refused, naming the rule it needs.
    Repair {
        /// The edge's name, as `provision` created it.
        name: String,
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
